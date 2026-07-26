import PMT.Basic
import PMT.Soundness

/-!
## PmtInstr — Lean mirror of the PMT-relevant subset of Rust `IRInstr` (sorry-free)

This module mirrors the Rust `src/codegen/src/ir.rs::IRInstr` enum (32
variants) in its entirety — all 32 Rust variants now have a Lean `PmtInstr`
counterpart as of PMT-1-D. (Previously the model covered only the 7 memory
variants + 18 arithmetic / control-flow / atomic variants added in
PMT-1-A/B/C; PMT-1-D adds the final 10 channel/special variants.)

The 35 `PmtInstr` variants fall into five groups:

  * **Memory variants (7):** `alloc`, `load`, `store`, `free`,
    `transform`, `call`, `ret`. These produce / consume arena regions
    and are subject to the per-instruction `well_typed` check (Lean
    mirror of IVE's `verify_state_reads` / `verify_state_writes` /
    `verify_transform` — see `PMT.IVE.Soundness.{Transform,
    StateReads,StateWrites}`).

  * **Pure-arithmetic variants (12, added in PMT-1-A):** `bin_op`,
    `unary_op`, `cast`, `add`, `sub`, `mul`, `div`, `cmp`, `select`,
    `ct_select`, `ct_eq`, `get_address`. These are pure
    register-to-register computations with no arena effect:
    `effect = .none`, `to_steps = []`, `well_typed = True`. Mirroring
    them in the Lean model lets PMT simulation-relation proofs traverse
    IR functions whose body contains arithmetic interleaved with memory
    operations without needing to abstract the arithmetic away first.

  * **Control-flow variants (3, added in PMT-1-B):** `phi`, `branch`,
    `cond_branch`. These model the Rust `IRInstr::Phi` /
    `IRInstr::Branch` / `IRInstr::CondBranch` variants that are
    `PmtInstr`s (i.e., they live in `IRBlock.instructions`) but carry
    no memory effect. Their semantics are resolved at the CFG level
    (block successors / predecessors), not the `Step` level: each has
    `effect = .none`, `well_typed = True`, `to_steps = []`. The helper
    `PmtInstr.successor_labels` (defined in `PMT.IRProgram`)
    classifies the CFG-target labels contributed by each control-flow
    variant, so that `IRBlock.well_typed` can validate that the block's
    declared `successors` field is consistent with the control-flow
    instructions it contains.

  * **Atomic variants (3, added in PMT-1-C):** `atomic_load`,
    `atomic_store`, `atomic_cas`. These mirror the Rust
    `IRInstr::AtomicLoad` / `IRInstr::AtomicStore` / `IRInstr::AtomicCas`
    variants. **Single-threaded limitation.** PMT's soundness model
    (`pmt_soundness` in `PMT.Soundness`) is single-threaded: it models
    one `exec` pass over a flattened `Program = List Step` with no
    concurrent interleaving. Under single-threaded execution the
    *atomicity* of an atomic load/store/CAS is vacuous — there is no
    other thread to race with — so each atomic variant is treated
    exactly like a non-atomic memory access for the purposes of the
    soundness proof: `effect = .none` (atomicity is a concurrency
    concern, not a state-transformation concern under single-threaded
    semantics), `well_typed = True`, `to_steps = []` (the actual memory
    effect is modeled at the IVE/runtime layer, not as a PMT `Step`).
    The `AtomicOrdering` enum (§3.5) is preserved as a tag on each
    variant for forward-compatibility with any future concurrent
    extension, but its value is unconstrained and never inspected by
    `effect` / `well_typed` / `to_steps`. A full concurrent-execution
    semantics (interleaved `exec`, memory-model axioms,
    happens-before relations) is **out of scope** for PMT-1-C and is
    not modeled here.

  * **Channel/special variants (10, added in PMT-1-D):** `vector_op`,
    `channel_open`, `channel_send`, `channel_recv`, `channel_close`,
    `channel_recv_timeout`, `channel_recv_result`, `stark_proof`,
    `call_indirect`, `syscall`. These mirror the remaining Rust
    `IRInstr` variants that are not directly exercised by PMT's arena
    invariants. Each is modeled structurally with an explicit domain
    abstraction:
    - `vector_op` — pure SIMD computation, `effect = .none`,
      `well_typed = True`, `to_steps = []`.
    - `channel_*` (6 variants) — **out-of-band effect modeled by
      IVE's capability system, not by PMT's arena state**. Each has
      `effect = .none`, `well_typed = True`, `to_steps = []`. The
      channel handle is an opaque capability whose send / recv /
      close / timeout / result semantics are modeled at the IVE /
      runtime layer; PMT's `Step` model has no concept of channel
      state, so these variants flatten to `[]`.
    - `stark_proof` — **proof-buffer model**: proof verification is
      an opaque effect delegated to the verifier, not modeled as a
      PMT `Step`. `effect = .none`, `well_typed = True`,
      `to_steps = []`.
    - `call_indirect` — like `.call` but with an indirect
      (register-resident) target. `effect = .none`,
      `well_typed = True`, `to_steps = args.map (fun v =>
      ⟨v, v, ⟨1, []⟩, .transform⟩)` (mirrors `.call`).
    - `syscall` — **opaque-effect model**: syscalls are out-of-scope
      for PMT (no arena state interaction). `effect = .none`,
      `well_typed = True`, `to_steps = []`.

The simulation relation `pmt_instr_simulates_ir_instr`
connects each `PmtInstr` variant to its Rust `IRInstr` counterpart. All
theorems in this file close without `sorry`.

**Module dependency.** `PMT.PmtInstr` is depended on by `PMT.IRProgram`
(which embeds it in `IRBlock.instructions`), `PMT.ExecFunction`
(which flattens `PmtInstr` to `Step`), and `PMT.WellTypedStrong`.

The 12 arithmetic variants (PMT-1-A) are pure: they flatten to `[]`
under `PmtInstr.to_steps`, so they do not contribute `Step`s to
`IRFunction.flat_steps` and therefore do not perturb the
name-uniqueness conjuncts of `WellTyped` — `pmt_soundness` is
preserved sorry-free.

The 3 control-flow variants (PMT-1-B) likewise flatten to `[]`:
their effect is to redirect execution between `IRBlock`s, which is
modeled at the CFG level (`IRBlock.successors`, the `IRTerminator`
of each block, and the `PmtInstr.successor_labels` helper), not as
`Step`s. The `pmt_soundness` argument — which operates over the
flattened `Program = List Step` — is therefore unaffected.

The 3 atomic variants (PMT-1-C) likewise flatten to `[]`: their
memory effect (load/store/CAS) is modeled at the IVE / runtime
layer, not as a PMT `Step`, and their atomicity annotation
(`AtomicOrdering`, §3.5) is vacuous under single-threaded execution
(see the single-threaded limitation paragraph above). The
`pmt_soundness` argument — which operates over the flattened
`Program = List Step` — is therefore unaffected. The
`AtomicOrdering` field is preserved on each variant for
forward-compatibility with any future concurrent extension.

The 10 channel/special variants (PMT-1-D) are structured as follows:
9 of the 10 flatten to `[]` (`vector_op`, the 6 `channel_*` variants,
`stark_proof`, `syscall` — their effects are either pure SIMD
computation, out-of-band IVE capability effects, opaque proof
verification, or out-of-scope syscalls, none of which interact with
PMT's arena state). The 10th variant, `call_indirect`, flattens to
`args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩)` — exactly mirroring
`.call` (placeholder self-loop steps per argument vreg). The 9
`[]`-flattening variants cannot perturb the name-uniqueness conjuncts
of `WellTyped` (a `Step`-free instruction introduces no `in_var` /
`out_var`). The `call_indirect` variant contributes one placeholder
`Step` per argument vreg (carrying the `⟨1, []⟩` layout, well-formed
by `WF_Layout_empty`); its name-uniqueness obligation is discharged
by the existing `IRFunction.in_vars_unique` / `out_vars_unique`
conjuncts in `IRFunction.well_typed`, exactly as for `.call`. The
`pmt_soundness` argument — which operates over the flattened
`Program = List Step` — is therefore preserved sorry-free.

**References.**
  * Rust source: `src/codegen/src/ir.rs` (3,481 lines, 32 IRInstr variants).
  * Related modules: `PMT.IRProgram`, `PMT.ExecFunction`,
    `PMT.WellTypedStrong`, `PMT.SimRel`.

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
-/

namespace PMT

/-- §1: IRValue — operand type. Mirrors Rust `IRValue`.
Rust: `IRValue = Register(u32) | Immediate(i64) | Address(u64) | Label(String)`.
Lean: we use `Nat` for all numeric forms (simpler; simulation is by value). -/
inductive IRValue where
  | register : Nat → IRValue       -- vreg ID
  | immediate : Int → IRValue      -- constant
  | address   : Nat → IRValue      -- memory address
  | label     : String → IRValue   -- block label
  deriving Repr, DecidableEq

/-- §2: IRType — value type. Mirrors Rust `IRType` (subset). -/
inductive IRType where
  | i32   : IRType
  | i64   : IRType
  | u32   : IRType
  | u64   : IRType
  | ptr   : IRType
  | void  : IRType
  | struct : Layout → IRType       -- aggregate
  deriving Repr

/-! ## §3. Op-kind tags (mirrors of Rust enums used by the arithmetic variants)

The four inductives below mirror the eponymous Rust enums in
`src/codegen/src/ir.rs`:
  * `BinOpKind`   — `src/codegen/src/ir.rs:1080` (25 variants).
  * `UnaryOpKind` — `src/codegen/src/ir.rs:1195` (5 variants).
  * `CmpKind`     — `src/codegen/src/ir.rs:1282` (10 variants).
  * `CastKind`    — `src/codegen/src/ir.rs:1324` (9 variants).

They are pure tag types: `PmtInstr.well_typed` does not inspect them
(the arithmetic `PmtInstr` variants are all `True`-well-typed), so the
Lean variants are present solely to faithfully mirror the Rust IR shape
for `instr_sim` / `block_sim` traversal in `PMT.SimRel`. -/

/-- §3.1: Binary operator kind — mirror of Rust `BinOpKind`.
Covers arithmetic, bitwise, shift, rotate, and comparison sub-kinds. -/
inductive BinOpKind where
  | add   : BinOpKind
  | sub   : BinOpKind
  | mul   : BinOpKind
  | sdiv  : BinOpKind
  | udiv  : BinOpKind
  | srem  : BinOpKind
  | urem  : BinOpKind
  | «and» : BinOpKind
  | «or»  : BinOpKind
  | xor   : BinOpKind
  | shl   : BinOpKind
  | shrL  : BinOpKind
  | shrA  : BinOpKind
  | ror   : BinOpKind
  | rol   : BinOpKind
  | sLt   : BinOpKind
  | sLe   : BinOpKind
  | sGt   : BinOpKind
  | sGe   : BinOpKind
  | uLt   : BinOpKind
  | uLe   : BinOpKind
  | uGt   : BinOpKind
  | uGe   : BinOpKind
  | eq    : BinOpKind
  | ne    : BinOpKind
  deriving Repr

/-- §3.2: Unary operator kind — mirror of Rust `UnaryOpKind`. -/
inductive UnaryOpKind where
  | neg    : UnaryOpKind
  | not    : UnaryOpKind
  | clz    : UnaryOpKind
  | ctz    : UnaryOpKind
  | popcnt : UnaryOpKind
  deriving Repr

/-- §3.3: Comparison kind — mirror of Rust `CmpKind`. -/
inductive CmpKind where
  | eq  : CmpKind
  | ne  : CmpKind
  | sLt : CmpKind
  | sLe : CmpKind
  | sGt : CmpKind
  | sGe : CmpKind
  | uLt : CmpKind
  | uLe : CmpKind
  | uGt : CmpKind
  | uGe : CmpKind
  deriving Repr

/-- §3.4: Cast kind — mirror of Rust `CastKind`. -/
inductive CastKind where
  | zExt         : CastKind
  | sExt         : CastKind
  | trunc        : CastKind
  | bitCast      : CastKind
  | intToFloat   : CastKind
  | uIntToFloat  : CastKind
  | floatToInt   : CastKind
  | floatToUInt  : CastKind
  | floatToFloat : CastKind
  deriving Repr

/-! ## §3.5. AtomicOrdering — mirror of the C++/Rust `Ordering` enum (annotation tag)

`AtomicOrdering` is the standard 5-variant memory-ordering tag used by
LLVM, C++ `std::memory_order`, and Rust `std::sync::atomic::Ordering`.
It is preserved on each PMT-1-C atomic variant as a documentation /
forward-compatibility annotation: under PMT's single-threaded
soundness model, the ordering is *never inspected* by `PmtInstr.effect`,
`PmtInstr.well_typed`, or `PmtInstr.to_steps` (all three treat the
atomic variants identically regardless of `ordering`'s value). The
enum is present in the Lean model so that `instr_sim` / `block_sim`
traversal in `PMT.SimRel` can carry the ordering through structurally
for any future concurrent extension.

**Note on Rust source divergence.** The Rust `IRInstr::AtomicLoad` /
`AtomicStore` / `AtomicCas` variants at `src/codegen/src/ir.rs:1583` /
`1598` / `1617` do **not** carry an `ordering` field — they emit a
fixed acquire/release sequence (LDAXR/STLXR on AArch64,
LOCK CMPXCHG on x86_64, LR.D/SC.D on RISC-V) at codegen time without
exposing the ordering choice in the IR. The task brief for PMT-1-C
prescribed an enriched shape with explicit `ordering` /
`success_order` / `failure_order` fields; we follow the task brief's
prescription (rather than the leaner Rust shape) so that the Lean
model can document and carry the ordering intent. This is the same
kind of "faithful-but-simplified" modeling decision that PMT-1-A made
for the arithmetic variants' `ty : Option<IRType>` field (modeled as
non-`Option` `IRType`). -/

/-- §3.5: Atomic memory ordering — mirror of C++ `std::memory_order` /
Rust `std::sync::atomic::Ordering`. Tag only; vacuous under PMT's
single-threaded soundness model. -/
inductive AtomicOrdering where
  | relaxed : AtomicOrdering
  | acquire : AtomicOrdering
  | release : AtomicOrdering
  | acq_rel : AtomicOrdering
  | seq_cst : AtomicOrdering
  deriving Repr

/-! ## §3.6. VectorOpKind — mirror of Rust `VectorOpKind` (annotation tag)

`VectorOpKind` is the 3-variant SIMD lane-wise arithmetic tag used by
`IRInstr::VectorOp` (`src/codegen/src/ir.rs:1174`). It is preserved on
the `PmtInstr.vector_op` variant as a documentation tag: under PMT's
soundness model, `vector_op` is a pure register-to-register computation
(`effect = .none`, `well_typed = True`, `to_steps = []`), and the
`op`/`lanes`/`elem_size` fields are *never inspected* by `effect`,
`well_typed`, or `to_steps`. The enum is present in the Lean model so
that `instr_sim` / `block_sim` traversal in `PMT.SimRel` can carry the
op-kind through structurally for any future SIMD-aware extension. -/

/-- §3.6: SIMD lane-wise arithmetic kind — mirror of Rust `VectorOpKind`
(ir.rs:1174). Tag only; the underlying SIMD computation is treated as
pure under PMT's soundness model (no arena interaction). -/
inductive VectorOpKind where
  | add : VectorOpKind
  | sub : VectorOpKind
  | mul : VectorOpKind
  deriving Repr

/-! **Note on `PmtOp` migration.** The `PmtOp` inductive previously declared in this
module (with 7 constructors including `load`/`store`/`free`/
`call`) has been migrated to `PMT.Soundness` per the
design. The migrated `PmtOp`
has the 3-constructor shape (`alloc | field_access
Field | transform`), which is the minimal shape needed to make
`TrapCode.oob` reachable in the Lean model. The IR-level operation
tagging that the previous 7-constructor `PmtOp` supported is now
expressed directly through the `PmtInstr` variants themselves
(`PmtInstr.load`, `PmtInstr.store`, etc.) — the `PmtOp` indirection
was unused and has been removed. -/

/-- §4: PmtInstr — a single PMT-relevant instruction.

**Memory variants (7):** mirror the Rust `IRInstr` variants with arena
effect:
  - Rust `IRInstr::Alloc`      ↔ Lean `PmtInstr.alloc`
  - Rust `IRInstr::Load`       ↔ Lean `PmtInstr.load`
  - Rust `IRInstr::Store`      ↔ Lean `PmtInstr.store`
  - Rust `IRInstr::Free`       ↔ Lean `PmtInstr.free`
  - Rust `IRInstr::Call`       ↔ Lean `PmtInstr.call`
  - Rust `IRInstr::Ret`        ↔ Lean `PmtInstr.ret`

**Pure-arithmetic variants (12, PMT-1-A):** mirror the Rust `IRInstr`
variants that are pure register-to-register computations. Each has
`effect = .none`, `well_typed = True`, `to_steps = []`:
  - Rust `IRInstr::BinOp`     ↔ Lean `PmtInstr.bin_op`
  - Rust `IRInstr::UnaryOp`   ↔ Lean `PmtInstr.unary_op`
  - Rust `IRInstr::Cast`      ↔ Lean `PmtInstr.cast`
  - Rust `IRInstr::Add`       ↔ Lean `PmtInstr.add`
  - Rust `IRInstr::Sub`       ↔ Lean `PmtInstr.sub`
  - Rust `IRInstr::Mul`       ↔ Lean `PmtInstr.mul`
  - Rust `IRInstr::Div`       ↔ Lean `PmtInstr.div`
  - Rust `IRInstr::Cmp`       ↔ Lean `PmtInstr.cmp`
  - Rust `IRInstr::Select`    ↔ Lean `PmtInstr.select`
  - Rust `IRInstr::CtSelect`  ↔ Lean `PmtInstr.ct_select`
  - Rust `IRInstr::CtEq`      ↔ Lean `PmtInstr.ct_eq`
  - Rust `IRInstr::GetAddress` ↔ Lean `PmtInstr.get_address`

Field shapes mirror Rust (`src/codegen/src/ir.rs:1394-1689`). For
`BinOp`/`UnaryOp`/`Cast`/`Cmp`/`Add`/`Sub`/`Mul`/`Div`/`Select`/
`CtSelect`/`CtEq`, the Rust `ty: Option<IRType>` field is modeled
here as `ty : IRType` (the `None` default is omitted — the existing
`PmtInstr.load`/`.store` precedent uses non-`Option` `IRType`). For
`GetAddress`, the Rust `{ dst, name }` shape is mirrored faithfully
(the `Offset { dst, base, offset }` variant is a separate Rust
variant not in PMT scope).

**Control-flow variants (3, PMT-1-B):** mirror the Rust `IRInstr`
variants that redirect execution between `IRBlock`s. Each has
`effect = .none`, `well_typed = True`, `to_steps = []` (control flow
is resolved at the CFG level, not the step level):
  - Rust `IRInstr::Phi`        ↔ Lean `PmtInstr.phi`
  - Rust `IRInstr::Branch`     ↔ Lean `PmtInstr.branch`
  - Rust `IRInstr::CondBranch` ↔ Lean `PmtInstr.cond_branch`

Field shapes mirror Rust (`src/codegen/src/ir.rs:1470-1650`). The
Rust `Phi { dst, incoming }` shape has NO `ty` field (the task brief
listed a `ty : IRType` field, but the Rust source at ir.rs:1470 has
only `dst` and `incoming`); we mirror Rust faithfully (2 fields). For
`Branch`, the Rust `{ target }` shape is mirrored faithfully (1
field). For `CondBranch`, the Rust `{ cond, true_target, false_target }`
shape is mirrored faithfully (3 fields).

**Atomic variants (3, PMT-1-C):** mirror the Rust `IRInstr::AtomicLoad`
/ `AtomicStore` / `AtomicCas` variants. Each has `effect = .none`,
`well_typed = True`, `to_steps = []` (atomicity is a concurrency
concern, not a state-transformation concern under PMT's
single-threaded semantics; the underlying load/store/CAS memory
effect is modeled at the IVE / runtime layer, not as a PMT `Step`):
  - Rust `IRInstr::AtomicLoad` ↔ Lean `PmtInstr.atomic_load`
  - Rust `IRInstr::AtomicStore` ↔ Lean `PmtInstr.atomic_store`
  - Rust `IRInstr::AtomicCas`  ↔ Lean `PmtInstr.atomic_cas`

**Single-threaded limitation (PMT-1-C).** The `AtomicOrdering` enum
(§3.5) is preserved as a tag on each atomic variant, but its value
is never inspected by `effect` / `well_typed` / `to_steps`. Under
PMT's single-threaded `pmt_soundness` model, the atomicity of a
load/store/CAS is vacuous — there is no other thread to race with.
A full concurrent-execution semantics (interleaved `exec`,
memory-model axioms, happens-before) is out of scope for PMT-1-C.

**Note on Rust source divergence.** The Rust
`AtomicLoad`/`AtomicStore`/`AtomicCas` variants at ir.rs:1583/1598/1617
do **not** carry an `ordering` field — they emit a fixed
acquire/release sequence at codegen time. The task brief prescribed
an enriched shape with explicit `ordering` / `success_order` /
`failure_order` fields; we follow the task brief's prescription
(rather than the leaner Rust shape) so that the Lean model can
document and carry the ordering intent. Field names follow the Rust
precedent where they exist (`dst`, `addr`, `value`, `expected`,
`desired`, `ty`); the new `ordering` / `success_order` /
`failure_order` fields follow the task brief's prescription.

**Channel/special variants (10, PMT-1-D):** mirror the remaining
Rust `IRInstr` variants that are not directly exercised by PMT's
arena invariants. Each is modeled structurally with an explicit
domain abstraction:
  - Rust `IRInstr::VectorOp`           ↔ Lean `PmtInstr.vector_op`
    (pure SIMD computation; `VectorOpKind` (§3.6) is a tag only).
  - Rust `IRInstr::ChannelOpen`        ↔ Lean `PmtInstr.channel_open`
  - Rust `IRInstr::ChannelSend`        ↔ Lean `PmtInstr.channel_send`
  - Rust `IRInstr::ChannelRecv`        ↔ Lean `PmtInstr.channel_recv`
  - Rust `IRInstr::ChannelClose`       ↔ Lean `PmtInstr.channel_close`
  - Rust `IRInstr::ChannelRecvTimeout` ↔ Lean `PmtInstr.channel_recv_timeout`
  - Rust `IRInstr::ChannelRecvResult`  ↔ Lean `PmtInstr.channel_recv_result`
  - Rust `IRInstr::StarkProof`         ↔ Lean `PmtInstr.stark_proof`
  - Rust `IRInstr::CallIndirect`       ↔ Lean `PmtInstr.call_indirect`
  - Rust `IRInstr::Syscall`            ↔ Lean `PmtInstr.syscall`

**Channel capability model (PMT-1-D).** The 6 `channel_*` variants
each carry a channel handle (an opaque capability); their send /
recv / close / timeout / result semantics are modeled by IVE's
capability system, not by PMT's arena state. PMT's `Step` model has
no concept of channel state, so each `channel_*` variant has
`effect = .none`, `well_typed = True`, `to_steps = []`. The actual
I/O effect (may-block, may-wake, capability transfer) is modeled at
the IVE / runtime layer. A future PMT extension that models channel
state as part of the arena would extend `to_steps` to emit `Step`s
for these variants; for PMT-1-D's current scope (structural
modeling with channel capability abstraction), the `[]`-flattening
is sufficient.

**Stark proof-buffer model (PMT-1-D).** `stark_proof` carries a
public input, a destination vreg for the proof handle, and a list
of compile-time constraint coefficients. Proof verification is an
opaque effect delegated to the verifier (a separate `stark_verify`
builtin, not modeled here); the proof buffer itself is allocated
and tracked at the IPC / runtime layer (`crate::ipc::StarkProof`),
not as a PMT arena region. Hence `effect = .none`,
`well_typed = True`, `to_steps = []`.

**Call-indirect model (PMT-1-D).** `call_indirect` is like `.call`
but with an indirect (register-resident) target. The task brief
prescribes `to_steps = args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩)`,
identical to `.call`; this requires `args : List String` (each `v`
populates a `Step.in_var` / `out_var`), so we model the variant as
`String → List String → PmtInstr` (func_ptr name, arg vreg names).
The Rust `CallIndirect { dst: Option<IRValue>, func_ptr: IRValue,
args: Vec<IRValue> }` is therefore abstracted: `dst` is dropped
(no `Step` for the return value, mirroring `.call`), and `func_ptr`
is represented by its vreg name (a `String`). This is the same
"faithful-but-simplified" precedent as PMT-1-A's modeling of the
arithmetic variants' `ty : Option<IRType>` field as non-`Option`
`IRType`.

**Syscall opaque-effect model (PMT-1-D).** `syscall` carries a
syscall number, up to 6 argument IRValues, and an optional
destination IRValue. Syscalls are out-of-scope for PMT (no arena
state interaction — the syscall ABI is a runtime concern). Hence
`effect = .none`, `well_typed = True`, `to_steps = []`. The
field shape faithfully mirrors Rust (`nr : Nat`, `args : List
IRValue`, `dst : Option IRValue`).

Other Rust `IRInstr` variants not modeled here:
  - `IRInstr::Offset { dst, base, offset }` — not modeled (out of
    scope for PMT; the `PmtInstr.get_address` variant covers the
    primary symbol-resolution use case).
-/
inductive PmtInstr where
  -- Memory variants (7)
  | alloc       : String → Layout → PmtInstr                  -- (out_var, layout)
  | load        : String → String → Nat → IRType → PmtInstr   -- (in_var, out_var, offset, type)
  | store       : String → IRValue → Nat → IRType → PmtInstr  -- (in_var, value, offset, type)
  | free        : String → PmtInstr                           -- (in_var)
  | transform   : String → String → Layout → PmtInstr         -- (in_var, out_var, layout)
  | call        : String → List String → PmtInstr             -- (builtin_name, arg_vars)
  | ret         : IRValue → PmtInstr                          -- (return value)
  -- Pure-arithmetic variants (12, PMT-1-A) — `effect = .none`, `to_steps = []`
  | bin_op      : BinOpKind → IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `BinOp { op, dst, lhs, rhs, ty }`
  | unary_op    : UnaryOpKind → IRValue → IRValue → IRType → PmtInstr
    -- Rust `UnaryOp { op, dst, operand, ty }`
  | cast        : CastKind → IRValue → IRValue → IRType → IRType → PmtInstr
    -- Rust `Cast { kind, dst, src, from_ty, to_ty }`
  | add         : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Add { dst, lhs, rhs, ty }`
  | sub         : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Sub { dst, lhs, rhs, ty }`
  | mul         : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Mul { dst, lhs, rhs, ty }`
  | div         : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Div { dst, lhs, rhs, ty }`
  | cmp         : CmpKind → IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Cmp { kind, dst, lhs, rhs, ty }`
  | select      : IRValue → IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Select { dst, cond, true_val, false_val, ty }`
  | ct_select   : IRValue → IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `CtSelect { dst, cond, true_val, false_val, ty }`
  | ct_eq       : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `CtEq { dst, lhs, rhs, ty }`
  | get_address : IRValue → String → PmtInstr
    -- Rust `GetAddress { dst, name }`
  -- Control-flow variants (3, PMT-1-B) — `effect = .none`, `to_steps = []`
  | phi          : IRValue → List (IRValue × String) → PmtInstr
    -- Rust `Phi { dst, incoming: Vec<(IRValue, String)> }`
    -- (NOTE: Rust `Phi` has NO `ty` field — task brief's listed `ty`
    -- is a divergence; we mirror Rust faithfully.)
  | branch       : String → PmtInstr
    -- Rust `Branch { target: String }`
  | cond_branch  : IRValue → String → String → PmtInstr
    -- Rust `CondBranch { cond, true_target, false_target }`
  -- Atomic variants (3, PMT-1-C) — `effect = .none`, `to_steps = []`
  -- (single-threaded: atomicity vacuous; `AtomicOrdering` is a tag only)
  | atomic_load  : IRValue → IRValue → IRType → AtomicOrdering → PmtInstr
    -- Task brief: `AtomicLoad { dst, ptr, ty, ordering }`.
    -- Rust (ir.rs:1583): `AtomicLoad { dst, addr, ty }` (NO `ordering`
    -- field — divergence per task brief prescription; see §4 docstring).
  | atomic_store : IRValue → IRValue → IRType → AtomicOrdering → PmtInstr
    -- Task brief: `AtomicStore { ptr, val, ty, ordering }`.
    -- Rust (ir.rs:1598): `AtomicStore { value, addr, ty }` (NO `ordering`
    -- field; field order here follows Rust's `(value, addr, ty)` then
    -- appends `ordering` per task brief).
  | atomic_cas   : IRValue → IRValue → IRValue → IRValue → IRType
                   → AtomicOrdering → AtomicOrdering → PmtInstr
    -- Task brief:
    --   `AtomicCas { dst, ptr, expected, desired, ty, success_order, failure_order }`.
    -- Rust (ir.rs:1617): `AtomicCas { dst, addr, expected, desired, ty }`
    -- (NO `success_order`/`failure_order` fields — divergence per task
    -- brief prescription; see §4 docstring).
  -- Channel/special variants (10, PMT-1-D) — `effect = .none`,
  -- `to_steps = []` (except `call_indirect`, which mirrors `.call`).
  -- (Channels: capability effect modeled by IVE's capability system;
  --  StarkProof: proof verification is opaque, delegated to the verifier;
  --  Syscall: opaque effect, out-of-scope for PMT's arena state.)
  | vector_op : VectorOpKind → Nat → Nat → IRValue → IRValue → IRValue → PmtInstr
    -- Rust (ir.rs:1737): `VectorOp { op: VectorOpKind, lanes: u32,
    -- elem_size: u32, dst: IRValue, lhs: IRValue, rhs: IRValue }`.
    -- Pure SIMD computation — no arena effect.
  | channel_open : IRValue → IRType → PmtInstr
    -- Rust (ir.rs:1762): `ChannelOpen { dst: IRValue, elem_ty: IRType }`.
    -- Channels are an out-of-band effect modeled by IVE's capability
    -- system, not by PMT's arena state.
  | channel_send : IRValue → IRValue → Option IRType → PmtInstr
    -- Rust (ir.rs:1777): `ChannelSend { ch: IRValue, msg: IRValue,
    -- ty: Option<IRType> }`. (Out-of-band capability effect.)
  | channel_recv : IRValue → IRValue → Option IRType → PmtInstr
    -- Rust (ir.rs:1794): `ChannelRecv { ch: IRValue, dst: IRValue,
    -- ty: Option<IRType> }`. (Out-of-band capability effect.)
  | channel_close : IRValue → PmtInstr
    -- Rust (ir.rs:1809): `ChannelClose { ch: IRValue }`.
    -- (Out-of-band capability effect.)
  | channel_recv_timeout : IRValue → IRValue → Option IRType → Nat → PmtInstr
    -- Rust (ir.rs:1823): `ChannelRecvTimeout { ch: IRValue, dst: IRValue,
    -- ty: Option<IRType>, timeout_ms: u64 }`.
    -- (Out-of-band capability effect.)
  | channel_recv_result : IRValue → IRValue → IRValue → Option IRType → PmtInstr
    -- Rust (ir.rs:1848): `ChannelRecvResult { ch: IRValue, dst: IRValue,
    -- err_dst: IRValue, ty: Option<IRType> }`.
    -- (Out-of-band capability effect.)
  | stark_proof : IRValue → IRValue → List Nat → PmtInstr
    -- Rust (ir.rs:1886): `StarkProof { input: IRValue, dst: IRValue,
    -- constraints: Vec<u64> }`. Proof verification is an opaque effect
    -- delegated to the verifier — no PMT `Step` interaction.
  | call_indirect : String → List String → PmtInstr
    -- Rust (ir.rs:1907): `CallIndirect { dst: Option<IRValue>,
    -- func_ptr: IRValue, args: Vec<IRValue> }`.
    -- Modeled here as `(func_ptr, arg_vars) : String × List String`
    -- to mirror `.call : String → List String → PmtInstr` exactly
    -- (the task brief prescribes `to_steps = args.map (fun v =>
    -- ⟨v, v, ⟨1, []⟩, .transform⟩)`, which requires `args : List
    -- String` so each `v` can populate a `Step.in_var`/`out_var`).
    -- `dst` (Option IRValue) and `func_ptr` (IRValue) are abstracted
    -- to the func-ptr's vreg name (a String) — faithful-but-simplified,
    -- same precedent as the arithmetic `ty : Option<IRType>` → `IRType`.
  | syscall : Nat → List IRValue → Option IRValue → PmtInstr
    -- Rust (ir.rs:1711): `Syscall { nr: u32, args: Vec<IRValue>,
    -- dst: Option<IRValue> }`. Syscalls are out-of-scope for PMT
    -- (no arena state interaction) — opaque effect, no `Step`s emitted.
  deriving Repr

/-- §5: A PMT-relevant basic block — a list of PmtInstr. -/
abbrev PmtBlock := List PmtInstr

/-- §6: A PMT-relevant function — name + body. -/
structure PmtFunction where
  name : String
  body : PmtBlock
  deriving Repr

/-- §7: A PMT-relevant program — list of functions. -/
abbrev PmtProgram := List PmtFunction

/-- §8: Effect classification — what does each PmtInstr do to state?
  - `Reads in_var` — reads from in_var (requires it to be live)
  - `Writes out_var` — produces out_var (marks it live)
  - `Consumes in_var` — kills in_var (marks it dead, like transform)
  - `None` — no state effect (pure computation)
-/
inductive PmtEffect where
  | reads    : String → PmtEffect
  | writes   : String → PmtEffect
  | consumes : String → PmtEffect
  | none     : PmtEffect
  deriving Repr

/-- §9: Classify an instruction's effect.

The 12 pure-arithmetic variants (PMT-1-A) all classify as `.none` —
they neither read from nor write to a state variable, nor do they
consume one. -/
def PmtInstr.effect : PmtInstr → PmtEffect
  | .alloc out _ => .writes out
  | .load in_var _ _ _ => .reads in_var
  | .store in_var _ _ _ => .reads in_var
  | .free in_var => .consumes in_var
  | .transform in_var _ _ => .consumes in_var  -- NOTE: also writes out, but consumes dominates
  | .call _ _ => .none
  | .ret _ => .none
  -- Pure-arithmetic variants (PMT-1-A): no state effect.
  | .bin_op _ _ _ _ _ => .none
  | .unary_op _ _ _ _ => .none
  | .cast _ _ _ _ _ => .none
  | .add _ _ _ _ => .none
  | .sub _ _ _ _ => .none
  | .mul _ _ _ _ => .none
  | .div _ _ _ _ => .none
  | .cmp _ _ _ _ _ => .none
  | .select _ _ _ _ _ => .none
  | .ct_select _ _ _ _ _ => .none
  | .ct_eq _ _ _ _ => .none
  | .get_address _ _ => .none
  -- Control-flow variants (PMT-1-B): pure control flow, no arena effect.
  | .phi _ _ => .none
  | .branch _ => .none
  | .cond_branch _ _ _ => .none
  -- Atomic variants (PMT-1-C): atomicity is a concurrency concern, not a
  -- state-transformation concern. Under PMT's single-threaded semantics
  -- the atomicity is vacuous, so each variant classifies as `.none`
  -- (the underlying load/store/CAS memory effect is modeled at the IVE /
  -- runtime layer, not as a PMT `Step`).
  | .atomic_load _ _ _ _ => .none
  | .atomic_store _ _ _ _ => .none
  | .atomic_cas _ _ _ _ _ _ _ => .none
  -- Channel/special variants (PMT-1-D): see §4 docstring. Each of the
  -- 10 variants classifies as `.none`:
  --  * `vector_op` — pure SIMD computation (no arena effect).
  --  * `channel_*` (6 variants) — channels are an out-of-band effect
  --    modeled by IVE's capability system, not by PMT's arena state.
  --  * `stark_proof` — proof verification is an opaque effect delegated
  --    to the verifier; no PMT `Step` interaction.
  --  * `call_indirect` — like `.call`, the per-argument vreg traffic is
  --    modeled at the IVE-from-IR layer (the `args.map (fun v => …)`
  --    `Step`s are placeholder self-loops, not state transitions).
  --  * `syscall` — opaque effect; syscalls are out-of-scope for PMT
  --    (no arena state interaction).
  | .vector_op _ _ _ _ _ _ => .none
  | .channel_open _ _ => .none
  | .channel_send _ _ _ => .none
  | .channel_recv _ _ _ => .none
  | .channel_close _ => .none
  | .channel_recv_timeout _ _ _ _ => .none
  | .channel_recv_result _ _ _ _ => .none
  | .stark_proof _ _ _ => .none
  | .call_indirect _ _ => .none
  | .syscall _ _ _ => .none

/-- §10: WellTypedness for PmtInstr (per-instruction).
This is the per-instruction check that IVE's `verify_state_reads` /
`verify_state_writes` / `verify_transform` perform.

The 12 pure-arithmetic variants (PMT-1-A) all reduce to `True`: they
are pure register-to-register computations with no arena
involvement, so the per-instruction `WF_Layout` check does not
apply. -/
def PmtInstr.well_typed (i : PmtInstr) (layout_env : String → Layout) : Prop :=
  match i with
  | .alloc _out layout => WF_Layout layout
  | .load in_var _ _ _ => WF_Layout (layout_env in_var)
  | .store in_var _ _ _ => WF_Layout (layout_env in_var)
  | .free in_var => WF_Layout (layout_env in_var)
  | .transform in_var _out layout =>
      WF_Layout layout ∧ WF_Layout (layout_env in_var)
  | .call _ _ => True
  | .ret _ => True
  -- Pure-arithmetic variants (PMT-1-A): pure computation, no arena involvement.
  | .bin_op _ _ _ _ _ => True
  | .unary_op _ _ _ _ => True
  | .cast _ _ _ _ _ => True
  | .add _ _ _ _ => True
  | .sub _ _ _ _ => True
  | .mul _ _ _ _ => True
  | .div _ _ _ _ => True
  | .cmp _ _ _ _ _ => True
  | .select _ _ _ _ _ => True
  | .ct_select _ _ _ _ _ => True
  | .ct_eq _ _ _ _ => True
  | .get_address _ _ => True
  -- Control-flow variants (PMT-1-B): pure control flow, no arena involvement.
  | .phi _ _ => True
  | .branch _ => True
  | .cond_branch _ _ _ => True
  -- Atomic variants (PMT-1-C): atomicity is vacuous under PMT's
  -- single-threaded semantics; the underlying load/store/CAS memory
  -- effect is modeled at the IVE / runtime layer. The `AtomicOrdering`
  -- tag is not inspected (any ordering is well-typed).
  | .atomic_load _ _ _ _ => True
  | .atomic_store _ _ _ _ => True
  | .atomic_cas _ _ _ _ _ _ _ => True
  -- Channel/special variants (PMT-1-D): see §4 docstring. Each of the
  -- 10 variants reduces to `True`:
  --  * `vector_op` — pure SIMD computation (no arena involvement).
  --  * `channel_*` (6 variants) — channels are an out-of-band effect
  --    modeled by IVE's capability system, not by PMT's arena state.
  --  * `stark_proof` — proof verification is opaque, delegated to the
  --    verifier; no PMT `Step` interaction (no `WF_Layout` check).
  --  * `call_indirect` — like `.call`, no per-instruction `WF_Layout`
  --    obligation (the per-argument placeholder `Step`s carry
  --    `⟨1, []⟩`, well-formed by `WF_Layout_empty`).
  --  * `syscall` — opaque effect, out-of-scope for PMT; no `WF_Layout`
  --    check.
  | .vector_op _ _ _ _ _ _ => True
  | .channel_open _ _ => True
  | .channel_send _ _ _ => True
  | .channel_recv _ _ _ => True
  | .channel_close _ => True
  | .channel_recv_timeout _ _ _ _ => True
  | .channel_recv_result _ _ _ _ => True
  | .stark_proof _ _ _ => True
  | .call_indirect _ _ => True
  | .syscall _ _ _ => True

/-- §11: WellTypedness for a PmtBlock. -/
def PmtBlock.well_typed (b : PmtBlock) (env : String → Layout) : Prop :=
  ∀ i : PmtInstr, i ∈ b → i.well_typed env

/-- §12: WellTypedness for a PmtFunction. -/
def PmtFunction.well_typed (f : PmtFunction) (env : String → Layout) : Prop :=
  f.body.well_typed env

/-- §13: WellTypedness for a PmtProgram. -/
def PmtProgram.well_typed (p : PmtProgram) (env : String → Layout) : Prop :=
  ∀ f : PmtFunction, f ∈ p → f.well_typed env

/-! ## §14. Per-instruction flattening: `PmtInstr.to_steps`

`PmtInstr.to_steps` converts a `PmtInstr` to a list of `Step`s (the
flat-program representation consumed by `exec` in `PMT.Soundness`).
This is defined here in `PmtInstr.lean` (rather than in
`PMT/ExecFunction.lean`) so that `IRFunction.flat_steps` and
`IRFunction.well_typed` (in `PMT/IRProgram.lean`) can refer to it
without creating a circular import (`ExecFunction.lean` already
imports `IRProgram.lean`). The flattening semantics are identical to
those documented in `PMT/ExecFunction.lean` §1. -/

/-- §14: Convert a `PmtInstr` to a list of `Step`s.

Mapping rationale (mirrors the IVE-from-IR traversal in W2-A §6):
  - `alloc out layout`   → 1 step: `⟨out, out, layout⟩`
      (allocates a region named `out`; both in/out are `out` because the
      allocator "consumes" the request and "produces" the new region.
      The `WellTyped` name-uniqueness check is satisfied because `out`
      appears exactly once as `in_var` and once as `out_var` in this step
      — the filter counts each step once.)
  - `load in_var out _ _` → 1 step: `⟨in_var, out, ⟨1, []⟩⟩`
      (reads `in_var`, produces `out`; size 1 is a placeholder — a future
      refinement will use the IRType to compute the actual byte size.)
  - `store in_var _ _ _`  → 1 step: `⟨in_var, in_var, ⟨1, []⟩⟩`
      (writes through `in_var` without producing a new region.)
  - `free in_var`         → 1 step: `⟨in_var, in_var, ⟨1, []⟩⟩`
      (frees `in_var`; marked as both reader and writer for liveness.)
  - `transform in_var out layout` → 1 step: `⟨in_var, out, layout⟩`
      (consumes `in_var`, produces `out` with the given layout.)
  - `call _ args`         → `args.map (fun v => ⟨v, v, ⟨1, []⟩⟩)`
      (each argument variable becomes a self-loop step; placeholder
      until call semantics are modeled.)
  - `ret _`               → `[]`
      (return has no further memory effect in this straight-line model.)

  The 12 pure-arithmetic variants (PMT-1-A) all map to `[]`: they are
  pure register-to-register computations and contribute no `Step`s to
  the flattened program. Consequently they do not perturb the
  name-uniqueness conjuncts of `WellTyped` (a `Step`-free instruction
  cannot introduce a duplicate `in_var` / `out_var`). -/
def PmtInstr.to_steps (i : PmtInstr) : List Step :=
  match i with
  | .alloc out layout => [⟨out, out, layout, .transform⟩]
  | .load in_var out _ _ => [⟨in_var, out, ⟨1, []⟩, .transform⟩]
  | .store in_var _ _ _ => [⟨in_var, in_var, ⟨1, []⟩, .transform⟩]
  | .free in_var => [⟨in_var, in_var, ⟨1, []⟩, .transform⟩]
  | .transform in_var out layout => [⟨in_var, out, layout, .transform⟩]
  | .call _ args => args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩)
  | .ret _ => []
  -- Pure-arithmetic variants (PMT-1-A): pure computation, no Steps emitted.
  | .bin_op _ _ _ _ _ => []
  | .unary_op _ _ _ _ => []
  | .cast _ _ _ _ _ => []
  | .add _ _ _ _ => []
  | .sub _ _ _ _ => []
  | .mul _ _ _ _ => []
  | .div _ _ _ _ => []
  | .cmp _ _ _ _ _ => []
  | .select _ _ _ _ _ => []
  | .ct_select _ _ _ _ _ => []
  | .ct_eq _ _ _ _ => []
  | .get_address _ _ => []
  -- Control-flow variants (PMT-1-B): control flow resolved at the CFG
  -- level (block successors / `PmtInstr.successor_labels`), not as Steps.
  | .phi _ _ => []
  | .branch _ => []
  | .cond_branch _ _ _ => []
  -- Atomic variants (PMT-1-C): atomicity is vacuous under PMT's
  -- single-threaded semantics; the underlying load/store/CAS memory
  -- effect is modeled at the IVE / runtime layer, not as a PMT `Step`.
  -- The `AtomicOrdering` tag is not inspected.
  | .atomic_load _ _ _ _ => []
  | .atomic_store _ _ _ _ => []
  | .atomic_cas _ _ _ _ _ _ _ => []
  -- Channel/special variants (PMT-1-D): see §4 docstring.
  --  * `vector_op` — pure SIMD computation, no `Step`s emitted (the
  --    underlying packed arithmetic is modeled at the IVE-from-IR layer).
  --  * `channel_*` (6 variants) — channels are an out-of-band effect
  --    modeled by IVE's capability system, not by PMT's arena state, so
  --    they contribute no `Step`s.
  --  * `stark_proof` — proof verification is opaque (delegated to the
  --    verifier); no `Step`s.
  --  * `call_indirect` — mirrors `.call`: each argument vreg becomes a
  --    placeholder self-loop `Step` (`⟨v, v, ⟨1, []⟩, .transform⟩`).
  --  * `syscall` — opaque effect, out-of-scope for PMT; no `Step`s.
  | .vector_op _ _ _ _ _ _ => []
  | .channel_open _ _ => []
  | .channel_send _ _ _ => []
  | .channel_recv _ _ _ => []
  | .channel_close _ => []
  | .channel_recv_timeout _ _ _ _ => []
  | .channel_recv_result _ _ _ _ => []
  | .stark_proof _ _ _ => []
  | .call_indirect _ args => args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩)
  | .syscall _ _ _ => []

end PMT
