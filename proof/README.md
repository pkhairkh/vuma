# proof/ — VUMA Formal Verification Tree

The `proof/` directory holds **three distinct, non-overlapping** Lean
components. They are deliberately kept separate (different module roots,
different namespaces, no shared filenames) so that each can be built and
audited independently. Do **not** merge them — see "Why two arena trees?"
below.

## Directory layout

| Path        | Module root | Namespace | Role                                                                 |
|-------------|-------------|-----------|----------------------------------------------------------------------|
| `PMT/`      | `PMT.*`     | `PMT`     | **PMT verification library** — Programs-as-Memory-Transformations    |
| `Pmt/`      | `Pmt.*`     | `Pmt`     | **Arena simulation model** — faithful Lean mirror of the Rust arena  |
| `extracted/`| n/a (FFI)   | n/a       | **Lean FFI extraction stubs + C linkage** for `PMT.Extraction`       |

Sanity check (Wave 0-C): `comm -12 <(ls Pmt/) <(ls PMT/)` returns **empty**
— the two trees share no filenames, so there is no case-collision risk on
case-sensitive filesystems and no ambiguity in `import` lines
(`import Pmt.Model` vs `import PMT.Basic`).

---

## `PMT/` — PMT verification library

> Root module: [`PMT.lean`](PMT.lean) — *PMT — Programs as Memory
> Transformations*. Re-exports every submodule so callers can
> `import PMT` and access everything in the `PMT` namespace.

This is the **formal verification core**: a machine-checked proof that
the VUMA IR's memory-transforming execution is sound, plus its Iris-style
separation-logic layering and the extraction surface that lets the Rust
runtime call the proven checkers.

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
| `Extraction.lean`, `ExtractionLemmas.lean` | `@[export]` FFI surface + soundness theorems for the extracted checkers             |
| `PipelineSim.lean`                | End-to-end pipeline simulation                                                                    |
| `IVE/`                            | **I**ntermediate **V**erification **E**nvironment: `Soundness/` (StateWrites, StateReads, Transform, Composition), `PillarSoundness.lean` |
| `Iris/`                           | Iris-style separation logic: `HeapModel`, `CapBndInvariant`, `ArenaRes`, `LiveMirrorInvariant`, `GuardInvariant`, `Composition`, `WeakestPrecond`, `FractionalPerm`, `SepGenuine` |
| `FFI/PillarSoundness.lean`        | FFI variant of pillar soundness                                                                   |
| `Test/`                           | Test harnesses: `ValidProgram`, `UafProgram`, `OverflowProgram`, `EmptyProgram`, `MultiStepProgram`, `ArenaBasicSim`, `SorryFreeAudit`, `PropertyTests`, `EdgeCases`, `RealisticProgram` |

Imports inside this tree use the uppercase prefix, e.g.
`import PMT.Soundness`, `import PMT.IVE.Soundness.Transform`,
`import PMT.Iris.HeapModel`.

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

## `extracted/` — Lean FFI extraction stubs + C linkage

> See [`extracted/README.md`](extracted/README.md) for the full,
> up-to-date FFI-bridge status.

This directory holds the **Rust/C side** of the Lean↔Rust FFI bridge for
the PMT checkers proven in `PMT/Extraction.lean`. The Lean definitions
there (each with a machine-checked soundness theorem) are the formal
source of truth; the files here let the Rust runtime link against them.

| File           | Purpose                                                                                          |
|----------------|--------------------------------------------------------------------------------------------------|
| `pmt_check.rs` | Hand-translated Rust checkers mirroring `PMT/Extraction.lean`, plus the `extern "C"` declarations for the 7 `@[export]`-ed Lean symbols (`lean_verified_{capacity,field_bounds,linearity,pmt}_check`, `lean_verify_{transform,state_reads,state_writes}`). Parity-tested against the Lean semantics. |
| `lean_stub.c`  | **Linkage stub** (compiled by `build.rs` into `liblean_extraction.a`) defining the 7 Lean `@[export]` symbols so the Rust binary links cleanly when the real Lean→C extraction pipeline is unavailable. Returns hardcoded placeholders; never read in production because `build.rs` does not emit `lean_ffi_linked` on the stub path. |
| `README.md`    | Honest per-wave status of the FFI bridge (Waves 4-A → 5-C): what is wired, what is stubbed, what is the production path. |

As of Wave 5 the bridge is **wired end-to-end but running on a stub**:
the real Lean runtime is not yet linked, so every FFI call resolves to a
fail-closed C stub and the hand-written Rust verifiers in `pmt_check.rs`
remain the production path.

---

## Why two arena trees? (`PMT/` vs `Pmt/`)

They are **not duplicates** — they serve different roles and must not be
merged:

- **`PMT/`** is the *large* verification library: full IR semantics,
  Iris separation logic, IVE soundness, pipeline simulation, and the
  `@[export]` FFI extraction surface consumed by `extracted/`. It proves
  the compiler's memory transformations sound at the IR level.
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
cd proof && lake build PMT     # the verification library
cd proof && lake build Pmt     # the arena simulation model
```
