# VUMA — Verified-Unsafe Memory Access

**Version 0.2.0-alpha.1**

VUMA is a statically-typed systems programming language and compiler framework
whose distinguishing feature is **behavioral contract verification at compile
time** — discharged by a hard-wired Z3 SMT solver — augmented by **mandatory
runtime bounds checks on arena-allocated accesses**. Bounds checks are always
on; the runtime memory-safety overhead they incur is intentionally **paid**,
not avoided. Source programs are parsed to an AST, lifted to a Semantic Code
Graph (SCG), verified by the Invariant Verification Engine (IVE) — which
emits a `discharge_rate=N%` summary — lowered to a backend-neutral IR,
optimized, register-allocated, and emitted as native object code (ELF) or
WebAssembly.

The compiler targets **19 production backends, all at 100%** — 29 944 of
29 944 tests pass across the full backend × test matrix — spanning x86, ARM,
RISC-V, MIPS, PowerPC, s390x, SPARC, LoongArch, Alpha, HPPA, M68K, and
wasm32. Programs are modeled as **Programs as Memory Transformations (PMT)**
— typed state transformations on a single backing arena — and verified
against liveness, exclusivity, origin, interpretation, and cleanup
invariants before code emission. Contracts (`requires` / `ensures`) and
`prove` blocks are discharged by **Z3**, integrated as a hard dependency;
the former Lean FFI bridge has been **removed** and replaced by hand-written
Rust verifiers driving Z3.

---

## 1. Key Metrics

| Metric | Value |
|--------------------------------|------------------------------------|
| Backends | 19 (all at 100%) |
| Tests passing | 29 944 / 29 944 across the full backend × test matrix |
| Contract / invariant discharge | Z3 SMT solver (hard dependency) |
| Real linear-scan regalloc | x86_64, aarch64, riscv64, ppc64 |
| Implementation language | Rust |
| License | MIT |
| Minimum Rust toolchain | nightly-2026-03-01 (rustc 1.87+) |

---

## 2. Feature List

### Language
- **`transform` is the only function keyword.** A VUMA program is a
  composition of named transforms over typed state.
- **`Result` + `?` operator** for error handling — no exceptions, no panics
  on the happy path.
- **`requires` / `ensures` contracts** on every transform, discharged by Z3
  at compile time.
- **`prove` blocks** for inline assertions that Z3 must discharge before
  code emission.
- **`#[secret]` labels** drive a real information-flow verifier (not a
  stub): the lattice check operates over real vregs, not source names.
- **Session types** are checked by a real session-type verifier that tracks
  real vregs across channel operations.

### Memory model
- **PMT (Programs as Memory Transformations)** — every program is a typed
  state-transformation over a single backing arena.
- **Mandatory runtime bounds checks** on every arena-allocated access
  (`UGe` against the arena length, trap via `__oob_trap`, exit code 134).
  The overhead is paid; raw-pointer arithmetic and `length_expr=None`
  accesses remain unchecked.
- **Two-pipe channel architecture** with a handle registry: one pipe for
  data, one for protocol continuation; the registry maps opaque handles to
  live channel state.

### Verification (IVE)
- **Z3 SMT solver is a hard dependency.** Contracts, `prove` blocks,
  session-type linearity, information-flow lattice, and PMT invariants are
  all discharged by Z3 — no Lean FFI bridge, no `sorry` stubs.
- **Hand-written Rust verifiers** drive Z3 directly. The verifiers emit
  SMT constraints, call Z3, and report discharge status per obligation.
- **IVE output reports `discharge_rate=N%`** — the fraction of obligations
  Z3 discharged automatically.
- **Proof-directed compilation**: the `LinearityReport` from the
  session-type verifier is wired into codegen — non-linear programs are
  rejected before register allocation.
- **ISA encodings verified against official manuals** for every backend
  (Intel SDM, ARM ARM, RISC-V ISA spec, MIPS64, Power ISA, s390x POP,
  SPARC V9, LoongArch, Alpha, PA-RISC, M68K, WebAssembly).

### Codegen
- **Real linear-scan register allocator** on x86_64, aarch64, riscv64, and
  ppc64. Other backends use a simpler spiller.
- **19 native + Wasm backends**, all at 100% test pass.
- **wasm32 fork emulation** — `spawn_worker` runs parent and child
  branches sequentially in a single Wasm process. This is **not** process
  isolation; the limitation is documented in §6.

---

## 3. Architecture

VUMA is organized as a 10-stage pipeline — **parse → AST → SCG → IVE → IR →
channel-lowering → opt → regalloc → backend → ELF/Wasm** — implemented as a
Cargo workspace of Rust crates plus an LSP module and a package manager.

```
        ┌─────────┐   ┌─────┐   ┌─────┐   ┌─────┐   ┌────┐
source →│ parser  │→ │ AST │ → │ SCG │ → │ IVE │ → │ IR │ → …
        └─────────┘   └─────┘   └─────┘   └──┬──┘   └────┘
                                            │
                                  ┌─────────┴─────────┐
                                  │ Z3 (hard dep)     │
                                  │ Rust verifiers    │
                                  │ discharge_rate=N% │
                                  └───────────────────┘
```

The default verification discipline (`VerificationLevel::Pmt`) treats every
program as a state-transformation over a typed backing arena and runs the
PMT state verifiers; the five legacy pointer invariants (liveness,
exclusivity, interpretation, origin, cleanup) are available at higher
verification levels. The full architecture, crate inventory, backend
matrix, and verification pipeline are documented in
[docs/caveats.md](docs/caveats.md).

### Workspace crates

| Crate | Role |
|----------------|------------------------------------------------------|
| `vuma-parser` | Lexer + parser, source → AST |
| `vuma-scg` | AST → Semantic Code Graph |
| `vuma-ive` | Invariant Verification Engine; Z3 discharge; `discharge_rate` |
| `vuma-core` | IR, optimizer, channel lowering, handle registry |
| `vuma-codegen` | Regalloc, ISel, ELF/Wasm emission, runtime PMT checker |
| `vuma-bd` | Behavior descriptor / contract metadata |
| `vuma-package` | Package manager |
| `vuma-tests` | Gold-standard harness, KAT vectors, parity tests |

---

## 4. Quick Start

### 4.1 Build

Prerequisites:

- **Rust nightly-2026-03-01** (rustc 1.87+):
  `rustup toolchain install nightly-2026-03-01`
- **Z3** ≥ 4.12 (hard dependency — contracts and `prove` blocks are
  discharged by Z3 at compile time). Install via your system package
  manager (`apt install z3`, `brew install z3`) or download from
  <https://github.com/Z3Prover/z3>.
- **wasmtime** ≥ 47.0 (to execute `wasm32` binaries via
  `scripts/wasm32_runner.py`).
- **qemu-user-static** 7.2.0 — for cross-arch execution of the non-native
  backends (aarch64, arm, mips64, mipsel, mips64el, ppc64, ppc64le,
  riscv64, s390x, sparc64, m68k, hppa, alpha, loongarch64). The mips64
  backend emits a little-endian MIPS64 ELF; use `qemu-mips64el-static`.

```bash
# Clone and build the compiler front-end + codegen.
git clone https://github.com/pkhairkh/vuma
cd vuma
cargo build --profile release-fast --bin compile_dump --bin dump_ir
# The release-fast profile disables LTO for ~5× faster iteration builds;
# the resulting binaries land in target/release-fast/.
```

### 4.2 Compile and run a test program

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

The IVE prints a `discharge_rate=N%` summary on stderr at the end of
verification; failed obligations are listed with the failing constraint.

### 4.3 Run the gold-standard suite

```bash
# Full 29 944-run matrix across all 19 backends under QEMU + wasmtime.
bash scripts/pi5_test_suite.sh --workers 4
cat test_results/summary.json
```

---

## 5. Backend Matrix

All 19 backends pass 29 944 / 29 944 tests (100%).

| # | Backend | ISA | Endian | Regalloc | Notes |
|---|---------|-----|--------|----------|-------|
| 1 | `x86_64` | x86-64 | LE | linear-scan | Native on x86_64 hosts |
| 2 | `x86_32` | i386 | LE | spiller | |
| 3 | `aarch64` | A64 | LE | linear-scan | |
| 4 | `aarch64_be` | A64 | BE | spiller | Thin wrapper over `aarch64` |
| 5 | `arm32` | A32 | LE | spiller | |
| 6 | `armeb` | A32 | BE | spiller | Thin wrapper over `arm32` |
| 7 | `riscv64` | RV64 | LE | linear-scan | |
| 8 | `riscv32` | RV32 | LE | spiller | |
| 9 | `mips64` | MIPS64 | LE | spiller | Emits LE ELF; run via `qemu-mips64el-static` |
| 10 | `mips64be` | MIPS64 | BE | spiller | Thin wrapper over `mips64` |
| 11 | `ppc64` | Power ISA v3 | BE | linear-scan | |
| 12 | `ppc64le` | Power ISA v3 | LE | spiller | Thin wrapper over `ppc64` |
| 13 | `loongarch64` | LoongArch | LE | spiller | |
| 14 | `s390x` | z/Arch | BE | spiller | |
| 15 | `sparc64` | SPARC V9 | BE | spiller | |
| 16 | `alpha` | Alpha | LE | spiller | |
| 17 | `hppa` | PA-RISC | BE | spiller | |
| 18 | `m68k` | Motorola 68k | BE | spiller | |
| 19 | `wasm32` | WebAssembly | LE | spiller | In-process fork emulation (§6) |

ISA encodings for every backend are verified against the official
architecture manual (Intel SDM, ARM ARM, RISC-V ISA spec, MIPS64, Power
ISA, s390x POP, SPARC V9, LoongArch, Alpha Architecture Manual, PA-RISC
1.1, M68k, WebAssembly spec).

---

## 6. Verification

### 6.1 Z3 — hard dependency

Z3 is the single SMT backend for VUMA's verification obligations:

- **Contract discharge** — every `requires` / `ensures` clause on every
  `transform` is encoded to SMT-LIB and discharged by Z3 before code
  emission.
- **`prove` blocks** — inline assertions, same path.
- **Session-type linearity** — the session-type verifier tracks real
  vregs across channel ops on the two-pipe channel architecture; Z3
  discharges the linearity obligations and the result is wired into
  codegen via a `LinearityReport`.
- **Information-flow lattice** — operates over real `#[secret]` labels
  and real vregs; Z3 discharges the no-leak obligations.
- **PMT invariants** — capacity, field-bounds, liveness, exclusivity,
  origin, interpretation, cleanup — encoded and discharged by Z3.

The IVE prints `discharge_rate=N%` on stderr at the end of verification,
listing every obligation Z3 could not close automatically.

### 6.2 Rust verifiers (Lean FFI bridge removed)

The previous Lean 4 FFI bridge has been **removed**. Verification is now
performed by **hand-written Rust verifiers** that:

1. Walk the SCG / IR and emit SMT-LIB constraints.
2. Call Z3 in-process.
3. Collect `(discharged | unknown | failed)` per obligation.
4. Emit the `LinearityReport` and other reports that codegen consumes.

This removes the Lean trusted computing base, the `sorry` audit, and the
`lake build` cycle from the Rust dev loop. The `pmt-runtime-check` Cargo
feature remains as an opt-in for the pure-Rust PMT runtime checker
(`src/codegen/src/runtime/pmt_check.rs`), a parity-tested hand-translation
of the PMT invariants.

### 6.3 Proof-directed compilation

The `LinearityReport` produced by the session-type verifier is **wired
into codegen**: non-linear programs are rejected before register
allocation, so a failed linearity check is a compile error, not a runtime
trap. The same pattern is used for the information-flow verifier
(`#[secret]` leakage is a compile error) and the contract verifier
(`requires` / `ensures` failure is a compile error).

### 6.4 Building / running verification

```bash
# Discharge contracts + run IVE for a single source file.
./target/release-fast/compile_dump examples/fibonacci.vuma out.bin x86_64

# Run the gold-standard suite (discharges + executes 29 944 runs).
bash scripts/pi5_test_suite.sh --workers 4
```

No `lake build`, no `elan`, no Lean toolchain required.

---

## 7. Documentation Index

| Document | Scope |
|-----------------------------------------------------|----------------------------------------------------|
| [docs/caveats.md](docs/caveats.md) | 10-stage pipeline, crates, backend matrix, verification |
| [docs/architecture/](docs/architecture/) | Per-subsystem audits (IVE, PMT, IPC, caveats) |
| [docs/language-reference.md](docs/language-reference.md) | VUMA language reference: types, expressions, builtins |
| [docs/language/](docs/language/) | Tutorial, semantics, calling-convention notes |
| [docs/backends/](docs/backends/) | Per-backend ABI tables, ISel strategy, QEMU notes |
| [docs/testing/](docs/testing/) | Gold-standard harness, CI workflows, KAT vectors |
| [docs/building.md](docs/building.md) | Build prerequisites, quick start, troubleshooting |

The repository also ships reference material outside `docs/`:

- `examples/` — self-contained `.vuma` programs (atomics, channels,
  sha256d, lock-free queue, doubly-linked list, mmap + sha256d).
- `womb/` — VUMA standard library: alloc, collections, crypto (hashes,
  symmetric, asymmetric, post-quantum, MAC/KDF, bignum), encoding, env,
  fs, graph, io, kernel, net (TCP, TLS 1.2/1.3, SSH, QUIC, HTTP/2/3, DNS,
  websocket), string, syscalls.
- `tests/gold_standard/` — gold-standard programs; the manifest
  (`tests/gold_standard/manifest.json`) is the canonical source of truth
  (`make verify-manifest` to confirm, `make regen-manifest` to rebuild).
- `scripts/` — test runners (`pi5_test_suite.sh`, `wasm32_runner.py`),
  QEMU boot scripts, fuzz harnesses, KAT generators.

---

## 8. Caveats

VUMA is at `0.2.0-alpha.1`. Prospective users should read
[docs/caveats.md](docs/caveats.md) before relying on any feature; that
file is the canonical source of truth for all known stubs, partial
implementations, and architectural issues. Highlights to be aware of
up-front:

1. **PMT** in this codebase means "Programs as Memory Transformations,"
   not "Persistent Memory Transaction".
2. **Runtime bounds checks are always on.** Every arena-allocated access
   gets a `UGe` bounds check that traps via `__oob_trap` (exit 134). The
   runtime memory-safety overhead is **paid**, not avoided. Raw-pointer
   arithmetic and `length_expr=None` accesses remain unchecked (future
   SoftBound work).
3. **"No buffer overflow"** is true only for the arena bump pointer and
   the bounds-checked `Seq` accesses, not for arbitrary pointer
   arithmetic.
4. The four thin-wrapper backends (`aarch64_be`, `armeb`, `mips64be`,
   `ppc64le`) byte-swap around their LE/BE parent backends.
5. **`spawn_worker` on wasm32 emulates `fork(2)`** by running parent and
   child branches sequentially in a single Wasm process. This is **not**
   process isolation; the limitation is documented and a one-shot
   `vuma_log!(warn)` diagnostic is emitted on every emulated fork.
6. The canonical-pipeline `emit_elf` (`src/codegen/src/emit.rs`) is
   AArch64-only — non-AArch64 `--isa` values route through the direct
   AST→codegen bridge, which skips full IVE gating; use `vuma build` for
   that.
7. **Z3 is a hard dependency.** A missing or too-old Z3 is a build error,
   not a warning.

---

## 9. License

Copyright © 2026 VUMA Project Contributors. Released under the **MIT
License**; see [LICENSE](LICENSE) for the full text.
