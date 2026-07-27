/-
! # PMT — Programs as Memory Transformations

Root module for the VUMA PMT formal verification library.
Re-exports all submodules so callers can `import PMT` and access
everything in the `PMT` namespace.

Modules:
  - `PMT.Basic`     — §1-§2: Arena, Field, Layout, CapacityInvariant
  - `PMT.Field`     — §3-§4: FieldBounds, Linearity, LinearResource
  - `PMT.Liveness`  — §5-§6: Liveness predicate, GuardPage
  - `PMT.PmtInstr`  — §1-§13: Lean mirror of the PMT-relevant subset of Rust `IRInstr`
                      (Alloc/Load/Store/Free/Call/Ret; 6 of 32 IRInstr variants)
  - `PMT.Soundness` — §7:   Execution model, pmt_soundness theorem
  - `PMT.Test.ValidProgram`    — valid 2-step happy-path test (W7-A)
  - `PMT.Test.UafProgram`      — UAF-detection test harness (W7-B)
  - `PMT.Test.OverflowProgram` — arena-overflow test harness (W7-C)
  - `PMT.Test.EmptyProgram`    — empty-program nil-case test (W7-D)
  - `PMT.Test.MultiStepProgram` — multi-step capacity-preservation test (W7-E)
-/

import PMT.Basic
import PMT.Field
import PMT.Liveness
import PMT.PmtInstr
import PMT.IRProgram
import PMT.Soundness
import PMT.RawArena
import PMT.MmapArena
import PMT.BitVecArena
import PMT.ArenaProperties
import PMT.SimRel
import PMT.WellTypedStrong
import PMT.ExecFunction
import PMT.AdditionalTheorems
import PMT.IRLemmas
import PMT.IVE.Soundness.StateWrites
import PMT.IVE.Soundness.StateReads
import PMT.IVE.Soundness.Transform
import PMT.IVE.Soundness.Composition
import PMT.Extraction
import PMT.ExtractionLemmas
import PMT.MiscLemmas
import PMT.HelperLemmas
import PMT.PipelineSim
import PMT.Iris.HeapModel
import PMT.Iris.CapBndInvariant
import PMT.Iris.ArenaRes
import PMT.Iris.LiveMirrorInvariant
import PMT.Iris.GuardInvariant
import PMT.Iris.Composition
import PMT.Iris.WeakestPrecond
import PMT.Iris.FractionalPerm
import PMT.Test.ValidProgram
import PMT.Test.UafProgram
import PMT.Test.OverflowProgram
import PMT.Test.EmptyProgram
import PMT.Test.MultiStepProgram
import PMT.Test.ArenaBasicSim
import PMT.Test.SorryFreeAudit
import PMT.Test.PropertyTests
import PMT.Test.EdgeCases
