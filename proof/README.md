# proof/ — VUMA Formal Verification Tree

The `proof/` directory holds **three distinct, non-overlapping** Lean
components. They are deliberately kept separate (different module roots,
different namespaces, no shared filenames) so that each can be built and
audited independently. Do **not** merge them — see "Why two arena trees?"
below.

> **Role of the Lean proofs.** The Lean development under `proof/` is the
> **formal specification** of the PMT memory model. The Lean theorems are
> machine-checked (`lake build` passes; sorry-audit by
> `scripts/check_lean.sh`), but they are **not linked into the compiler
> binary**. Build-time and runtime verification go through **Z3** (the
> SMT solver, hard build-time dependency in `src/ive/Cargo.toml`:
> `z3 = "0.20"`) and the hand-written Rust verifiers in `src/ive/`. The
> IVE state verifiers emit `contract_assert(…)` obligations that Z3
> discharges at compile time; the current discharge rate is **100 % on
> the gold-standard suite (29 944 / 29 944 = 100.00 % across all 19
> backends)**. See [`../docs/caveats.md` §3](../docs/caveats.md) for the
> full separation statement.
>
> The previous Lean↔Rust FFI bridge has been **deleted**. There is no
> `lean_stub.c`, no `lean_ffi_linked` cfg, and no `lean_verify_*` /
> `lean_verified_*` extern surface. Z3 + the hand-written Rust verifiers
> are the executable verifier; the Lean proofs are the formal spec only.

## Directory layout

| Path        | Module root | Namespace | Role                                                                 |
|-------------|-------------|-----------|----------------------------------------------------------------------|
| `PMT/`      | `PMT.*`     | `PMT`     | **PMT verification library** — Programs-as-Memory-Transformations    |
| `Pmt/`      | `Pmt.*`     | `Pmt`     | **Arena simulation model** — faithful Lean mirror of the Rust arena  |
| `extracted/`| n/a (legacy FFI surface, no longer compiled) | n/a | **Legacy Lean FFI extraction stubs** — the FFI bridge is deleted; only `pmt_check.rs` (hand-translation) remains, parity-tested against the Lean definitions |

Sanity check: `comm -12 <(ls Pmt/) <(ls PMT/)` returns **empty**
— the two trees share no filenames, so there is no case-collision risk on
case-sensitive filesystems and no ambiguity in `import` lines
(`import Pmt.Model` vs `import PMT.Basic`).

---

## `PMT/` — PMT verification library (formal specification)

> Root module: [`PMT.lean`](PMT.lean) — *PMT — Programs as Memory
> Transformations*. Re-exports every submodule so callers can
> `import PMT` and access everything in the `PMT` namespace.

This is the **formal verification core**: a machine-checked proof that
the VUMA IR's memory-transforming execution is sound, plus its Iris-style
separation-logic layering and the extraction surface that *used to* let
the Rust runtime call the proven checkers (the FFI bridge is now deleted;
the hand-translated Rust checkers in `src/codegen/src/runtime/pmt_check.rs`
are parity-tested against these Lean definitions instead).

| Submodule                         | Contents                                                                                         |
|-----------------------------------|--------------------------------------------------------------------------------------------------|
| `Basic.lean`                      | §1-§2: Arena, Field, Layout, CapacityInvariant                                                   |
| `Field.lean`                      | §3-§4: FieldBounds, Linearity, LinearResource                                                    |
| `Liveness.lean`                   | §5-§6: Liveness predicate, GuardPage                                                             |
| `PmtInstr.lean`                   | §1-§13: Lean mirror of the PMT-relevant subset of Rust `IRInstr` (Alloc/Load/Store/Free/Call/Ret)|
| `IRProgram.lean`, `IRLemmas.lean` | IR program syntax + supporting lemmas                                                            |
| `Soundness.lean`                  | §7: Execution model, `pmt_soundness` theorem                                                     |
| `RawArena.lean`, `MmapArena.lean`, `BitVecArena.lean`, `ArenaProperties.lean` | Arena implementations + properties                  |
| `SimRel.lean`                     | Simulation relation tying IR steps to arena transitions                                          |
| `WellTypedStrong.lean`           | Strong well-typedness judgement                                                                   |
| `ExecFunction.lean`              | Function execution semantics                                                                      |
| `AdditionalTheorems.lean`, `MiscLemmas.lean`, `HelperLemmas.lean` | Cross-cutting lemmas                          |
| `PillarSoundness.lean`, `NoFFI.lean` | Pillar-level soundness and the no-FFI reduction                                               |
| `Extraction.lean`, `ExtractionLemmas.lean` | `@[export]` FFI surface (legacy — the FFI bridge is deleted; the Lean definitions remain the formal source of truth) + soundness theorems for the extracted checkers |
| `PipelineSim.lean`                | End-to-end pipeline simulation                                                                    |
| `IVE/`                            | **I**ntermediate **V**erification **E**nvironment: `Soundness/` (StateWrites, StateReads, Transform, Composition), `PillarSoundness.lean` |
| `Iris/`                           | Iris-style separation logic: `HeapModel`, `CapBndInvariant`, `ArenaRes`, `LiveMirrorInvariant`, `GuardInvariant`, `Composition`, `WeakestPrecond`, `FractionalPerm`, `SepGenuine` |
| `FFI/PillarSoundness.lean`        | FFI variant of pillar soundness (legacy — FFI bridge deleted)                                    |
| `Test/`                           | Test harnesses: `ValidProgram`, `UafProgram`, `OverflowProgram`, `EmptyProgram`, `MultiStepProgram`, `ArenaBasicSim`, `SorryFreeAudit`, `PropertyTests`, `EdgeCases`, `RealisticProgram` |

Imports inside this tree use the uppercase prefix, e.g.
`import PMT.Soundness`, `import PMT.IVE.Soundness.Transform`,
`import PMT.Iris.HeapModel`.

> The `@[export]` annotations in `Extraction.lean` are **legacy** —
> they were used by the previous Lean↔Rust FFI bridge to expose Lean
> symbols to the Rust runtime. The bridge has been deleted; the
> annotations are retained in the Lean source so the formal spec
> remains self-documenting (each `@[export]` marks a function whose
> soundness theorem is machine-checked), but no C archive is produced
> and no `extern "C"` bindings resolve against Lean symbols in the
> current build. The Rust runtime calls hand-translated checkers in
> `src/codegen/src/runtime/pmt_check.rs` instead, parity-tested
> against these Lean definitions.

---

## `Pmt/` — Arena simulation model

> Root module: [`Pmt.lean`](Pmt.lean) — *Pmt — Faithful Lean Model of the
> VUMA Compiler*.

This is the **faithful Lean mirror of the Rust arena runtime**: a
self-contained `USize`/`Arena`/`Layout` model, an 8-instruction IR
subset, a simulation relation proving Lean↔Rust agreement, and a
from-scratch separation-logic / CMRA / WP stack used to discharge
overflow and UAF safety. It is the "small model" companion to the
large `PMT/` verification library.

| Module               | Description                                                                                          |
|----------------------|------------------------------------------------------------------------------------------------------|
| `Model.lean`         | USize (Fin 2^64), Ptr (provenance), Arena (overflow-checked bump allocator), Layout (field disjointness) |
| `Agreement.lean`     | Agreement theorem: Lean Arena.alloc decision matches Rust mirror                                     |
| `IrSubset.lean`      | 8-instruction IR subset (alloc, free, stateRead, stateWrite, stateTransform, chanNew, chanSend, chanRecv) with 16-constructor Step relation |
| `Simulation.lean`    | sim_state relation + sim_alloc theorem (Lean↔Rust alloc agreement)                                   |
| `Simulation2.lean`   | sim_free + sim_read theorems                                                                         |
| `SimWrite.lean`      | sim_write theorem (stateWrite preserves env)                                                         |
| `SimTransform.lean`  | sim_transform theorem (linear move semantics)                                                        |
| `SimIpc.lean`        | sim_chan_new/send/recv theorems (channel operations)                                                 |
| `SimSound.lean`      | Top-level simulation theorem (3-instruction induction)                                               |
| `SimSound2.lean`     | Top-level simulation theorem (8-instruction, 118 tactic lines)                                       |
| `Sep.lean`           | Separation logic from scratch: HeapModel, sep (disjoint domains), Ptsto, FracPtsto                   |
| `CMRA.lean`          | CMRA class (core, valid, 3 laws), Excl, Auth resource algebras                                       |
| `WP.lean`            | Inductive weakest precondition (2 constructors: wp_done, wp_step) + wp_frame, wp_bind                |
| `FancyUpdate.lean`   | Fancy updates (P |==> Q), Inv, invariant opening/closing, cap_bnd_inv_alloc                          |
| `WPSafety.lean`      | wp_alloc_safe (wp_step + Step.alloc_ok), wp_safety                                                   |
| `ArenaInv.lean`      | [cap_bnd] invariant: CapBnd structure, cap_bnd_init, cap_bnd_alloc, cap_bnd_never_exceeds            |
| `GuardInv.lean`      | [guard] invariant: GuardInv structure, guard_and_cap_frame, guard_alloc_safe                         |
| `OverflowProof.lean` | Arena overflow safety with Fin(2^64) arithmetic (2 theorems, 15+ and 20+ tactic lines)               |
| `UafProof.lean`      | No-UAF with pointer aliasing: no_uaf_ptr, no_uaf_alias (handles y=x and y≠x cases)                   |
| `Extract.lean`       | Extract Arena.alloc to Rust string, prove extract_nonempty/has_overflow_check/has_capacity_check     |
| `ExtractCorrect.lean`| extraction_correct: Arena.alloc returns None iff overflow OR OOB (35 tactic lines)                   |
| `RustConformance.lean`| Rust conformance lemmas for the arena model                                                         |

Imports inside this tree use the lowercase prefix, e.g.
`import Pmt.Model`, `import Pmt.Sep`, `import Pmt.SimSound2`.

All `Pmt/` modules are sorry-free — `grep -rc sorry Pmt/` returns 0 for
every file.

---

## `extracted/` — Legacy Lean FFI extraction stubs (bridge deleted)

> See [`extracted/README.md`](extracted/README.md) for the full
> historical record of the FFI-bridge status. The bridge has been
> **deleted**; the directory now holds only the hand-translated Rust
> checkers and a README documenting the historical context.

This directory previously held the **Rust/C side** of the Lean↔Rust FFI
bridge for the PMT checkers proven in `PMT/Extraction.lean`. The Lean
definitions there (each with a machine-checked soundness theorem) remain
the formal source of truth; the FFI bridge that used to let the Rust
runtime link against them has been **deleted**.

| File           | Purpose                                                                                          |
|----------------|--------------------------------------------------------------------------------------------------|
| `pmt_check.rs` | Hand-translated Rust checkers mirroring `PMT/Extraction.lean`. Parity-tested against the Lean semantics via `tests/pmt_parity_test.rs`. This is the **only file still consumed** — gated by the `pmt-runtime-check` Cargo feature on `vuma-codegen`. |
| `lean_stub.c`  | **No longer compiled.** The previous linkage stub (compiled by `build.rs` into `liblean_extraction.a`) defining the 7 Lean `@[export]` symbols so the Rust binary linked cleanly when the real Lean→C extraction pipeline was unavailable. **Removed** when the FFI bridge was deleted; the file is retained in-tree for historical reference only and is not part of any build target. |
| `README.md`    | Historical record of the FFI-bridge status (now closed — bridge deleted, Z3 replaces Lean as the executable verifier). |

As of the bridge deletion, **there is no FFI surface between Lean and
Rust**. The executable verifier is Z3 + the hand-written Rust verifiers
in `src/ive/`; the Lean proofs are the formal specification only. The
hand-translated Rust checkers in `pmt_check.rs` remain parity-tested
against the Lean definitions but are not themselves formally verified.

---

## Why two arena trees? (`PMT/` vs `Pmt/`)

They are **not duplicates** — they serve different roles and must not be
merged:

- **`PMT/`** is the *large* verification library: full IR semantics,
  Iris separation logic, IVE soundness, pipeline simulation, and the
  `@[export]` FFI extraction surface (legacy — bridge deleted) consumed
  by `extracted/pmt_check.rs` (hand-translation, parity-tested). It
  proves the compiler's memory transformations sound at the IR level.
- **`Pmt/`** is the *small* faithful model: a minimal Lean mirror of the
  Rust arena runtime (`USize`, `Arena`, `Layout`) plus a from-scratch
  separation-logic stack used to discharge concrete overflow/UAF safety
  and Lean↔Rust agreement theorems.

The two have **different module roots** (`PMT.*` vs `Pmt.*`), **different
namespaces** (`PMT` vs `Pmt`), and **no shared filenames**, so they
coexist safely under one `proof/` tree without collision. Merging them
would entangle two independently-auditable proof efforts and break the
existing `import` graph.

## Build

Both module roots build from this single `lakefile.toml`. To build a
specific tree:

```
cd proof && lake build PMT     # the verification library (formal spec)
cd proof && lake build Pmt     # the arena simulation model
```

CI runs `lake build` on every push via
`.github/workflows/proof-verify.yml` to confirm the formal Lean
specification still builds and is sorry-free (`scripts/check_lean.sh`).
This CI job gates the *formal spec* only — it does not gate the compiler
build, which is gated by the regular `ci.yml` build / test jobs that
exercise the Z3-backed executable verifier.
