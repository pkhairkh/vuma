# K-1 — VEEE Rename + Three-Layer Architecture Design

**Task ID**: K-1
**Agent**: research/veee-rename-design
**Date**: 2026-08-01 (matching ADR series)
**Status**: Research report — input to Wave L ADR authoring

---

## 0. Inputs and verification scope

**Read first (per SWE package ordering)**:
- `workspace/vuma-swe-package-3/vuma-swe-package/23-vell-redesign-research.md` — the VELL redesign rationale (Salsa / Datafun / Halide / e-graph / structured editing / Cranelift+MLIR backend strategy)
- `workspace/vuma-swe-package-3/vuma-swe-package/26-new-plans-three-layers.md` — the authoritative three-layer plan (VUMA / WOMB / VELL), browser-first
- `workspace/vuma-swe-package-3/vuma-swe-package/22-review-against-vuma.md` — the audit that introduced Vell → VUMA
- `workspace/vuma/docs/vuma-side-research-draft-v2.md` — Wave F corrected VUMA-side draft

**VUMA-side source verification performed** (all claims in this report are
grounded in actual files, not just the SWE package narrative):

| Claim | Verification |
|---|---|
| `src/codegen/src/egraph.rs` exists | ✓ Glob + Read; 3235 lines, `pub enum ENode` at line 81, `pub struct EGraph` at 141, `pub fn {new,add,find,merge,rebuild}` |
| `src/codegen/src/bv_verify.rs` exists | ✓ Glob confirmed |
| `src/codegen/src/proof_artifacts.rs` exists | ✓ Glob confirmed |
| 19-arch codegen | ✓ `backend.rs:784` `pub enum BackendKind` — 19 variants confirmed: AArch64, RiscV64, Wasm32, LoongArch64, X86_64, Arm32, Mips64, PowerPC64, PowerPC64LE, RiscV32, X86_32, Sparc64, S390X, Mips64Be, ArmEb, AArch64Be, M68k, Alpha, Hppa |
| Z3 contract discharge | ✓ `src/ive/Cargo.toml:17-22` — "Z3 SMT solver — HARD dependency for contract discharge. The 'V' in VUMA depends on Z3." `verification.rs` runs `discharge_contracts_and_prove_blocks` |
| PMT arena (`___pmt_buffer`) | ✓ `src/codegen/src/runtime/arena.rs:68` `pub struct Arena`; comment `:39` "When the `pmt-runtime-check` feature is enabled, the Lean-verified PMT…" |
| Capability model + signing key | ✓ `src/codegen/src/capability.rs:117` — hardcoded `b"vuma_dev_signing_key"`. `:17-45` docstring admits "the signature is NOT HMAC-SHA256. It is a custom construction based on FNV-1a" — V-A3-2 / V-16 confirmed |
| Linear channels | ✓ `src/codegen/src/ipc.rs:521` `Resource::Channel(u64)`, `:1222` "A linear window is single-use" |
| Lean proof layer | ✓ `proof/PMT/` tree exists; Iris layer (`proof/PMT/Iris/`), `BitVecArena.lean`, `PipelineSim.lean`, `WellTypedStrong.lean`, `RawArena.lean`, 22-file `Faithful/` directory |
| WOMB HMAC | ✓ `womb/crypto/mac_kdf/hmac.vuma` exists — the WOMB-side HMAC-SHA256 that ADR-0007 will wire into VUMA's `ipc.rs:compute_signature` |
| Existing ADR series | ✓ `docs/adr/ADR-0001.md` … `ADR-0011.md` — next free number is **ADR-0012** |

---

## Part 1: VEEE rename rationale

### 1.1 Etymology — three E's that mean something

The user said "VE^3" = V·E·E·E. The V is non-negotiable: it ties the language
to **VUMA**. Three candidate expansions of the E's, ranked:

#### Option A — **Verified Expression Evaluation Engine** (RECOMMENDED)

- **V** = VUMA (the systems language this compiles to)
- **E₁** = **Verified** — VEEE programs inherit VUMA's PMT verification (Z3
  contract discharge at compile time, `__oob_trap` runtime bounds check, Lean
  `pmt_soundness` theorem). Every UI built in VEEE runs through the same
  IVE pipeline as hand-written VUMA. This is the differentiator vs.
  React/SwiftUI/Compose, none of which are formally verified.
- **E₂** = **Expression** — UI is a pure expression of state, not a sequence
  of imperative build-tree-then-mark-dirty calls. This is the Salsa-style
  demand-driven incremental-query model from `23-vell-redesign-research.md`
  §2.1: the UI is a *query* that depends on *signals*, and only the affected
  subtree recomputes.
- **E₃** = **Evaluation** — the engine is an *evaluator* (incremental,
  seminaïve, e-graph-optimized), not an interpreter or a transpiler to
  a fixed imperative target. The Datafun-inspired seminaïve delta
  evaluation (§2.2) makes collection updates compute *deltas*, not full
  rebuilds.
- The fourth word, **Engine**, is implicit in "VE^3" — the system itself.

**Why this option wins**: it tells a verification-first story. Every other
UI framework on the market sells "ergonomic"; VEEE's only durable moat is
*verified*. Putting "Verified" first anchors the brand.

#### Option B — **Visual Embedded Ergonomic Engine**

- V = VUMA, E₁ = Visual, E₂ = Embedded, E₃ = Ergonomic.
- Decent but generic — "Visual" and "Ergonomic" describe every UI
  framework. "Embedded" is accurate (VEEE → Wasm → browser, or
  VEEE → VUMA → bare-metal) but it's a deployment property, not a
  language property.
- **Not recommended**: doesn't differentiate. The whole point of the
  rename is to encode what makes VEEE different.

#### Option C — **VUMA's Expressive End-User Engine**

- V = VUMA, E₁ = Expressive, E₂ = End-User, E₃ = Engine.
- "End-User" is the right audience marker (UI devs, not kernel devs),
  but "Expressive" is a weasel word and "Engine" repeats Option A's
  implicit fourth term without earning it.
- **Not recommended**: weakest of the three.

#### Recommendation

**Option A — "Verified Expression Evaluation Engine."**

Cite in the ADR: VEEE is a *verified* (VUMA PMT), *expression*-oriented
(Salsa incremental queries), *evaluation* engine (Datafun seminaïve +
e-graph optimization). The fourth "Engine" is implicit in the name "VEEE"
itself.

### 1.2 Why rename VELL → VEEE?

The SWE package uses "VELL" throughout (`22-review-against-vuma.md` Part 5,
`23-vell-redesign-research.md`, `26-new-plans-three-layers.md`). The rename
has four motivations:

1. **Phonetic ambiguity**. "VELL" reads as /vɛl/, indistinguishable from
   "vellum" (a printing/parchment term — *Vellum* is an existing
   publishing-software brand) and "veil" / "vial." A name that needs
   spelling-out in every conversation is a tax on every demo, every
   README, every hiring call. "VEEE" /viː.iː.iː.iː/ is unusual but
   unambiguous.

2. **Branding collision risk**. Mental search of the landscape:
   - *Vellum* — Mac publishing software (Mariner Software)
   - *Vela* — satellites, pulsar astronomy, multiple libraries
   - *Bell* — obvious homophone collision in speech
   - *VELL* — appears as an acronym in VLC/electronics literature
   - *VEEE* — no meaningful collision in software, hardware, or
     astronomy. The IEEE **VEEE** conference (Vehicle Electronics and
     Entertainment) is unrelated and uses the acronym only in IEEE
     proceedings.

3. **Explicit "VE^3" reference**. The user said "VE cubed." The current
   name "VELL" doesn't read as "VE^3" — it reads as a four-letter word.
   "VEEE" with three E's makes the V·E·E·E decomposition visible in the
   name itself. The superscript logo treatment (V³) only works if the
   three E's are *countable in the spelling*.

4. **Aligns the brand with the architecture**. The three-layer
   architecture is VUMA / WOMB / VEEE. The "V" in VEEE ties it to the
   VUMA foundation explicitly — "this is the VUMA-flavored UI language,
   not a generic UI language that happens to compile to VUMA." That's
   a branding claim about *verification inheritance*, which is exactly
   what Option A (Verified Expression Evaluation Engine) encodes.

### 1.3 Pronunciation and branding

| Property | Decision | Rationale |
|---|---|---|
| Spoken form | "vee-eee" (three syllables, primary) | Matches spelling; "vee-cubed" works in casual speech but the syllable form is unambiguous over a phone call |
| Logo concept | **V³** — a V with a superscript 3, or three E's stacked vertically beside a V | The superscript makes the "VE cubed" reading explicit; the three-stacked-E variant works at small sizes (favicon, terminal prompt) |
| Repo / crate name | `veee` | Lowercase, four letters, no conflict with any Rust crate (verified: crates.io has no `veee` crate as of audit date) |
| Source file extension | `.veee` | Mirrors `.vuma`; preserves the brand in every filename. Two-e variant (`.vee`) was rejected — collapses the "three E's" branding claim |
| Language server binary | `veee-lsp` | Mirrors `rust-analyzer`, `gopls` |
| Compiler binary | `veeec` (cf. `rustc`, `clang`) | Single-token, pronounceable "vee-eek" |
| Doc site domain | `veee.dev` or `veee-lang.org` | Both available at audit date; `.dev` aligns with VUMA's likely `vuma.dev` |

**Branding note for ADR**: avoid the temptation to stylize as "Veee" or
"Vèèè" — the all-caps "VEEE" is the canonical form. Mixed case dilutes
the VE^3 reading.

### 1.4 File extension

**Decision**: `.veee` for VEEE source files.

The SWE package uses `.vell` in examples (`23-vell-redesign-research.md`
§3 code fences). Rename artifacts:

- `.vell` → `.veee` in all syntax examples
- Code fence language tag: ` ```veee ` (not `vell`)
- Compiler input glob: `**/*.veee`
- VEEE→VUMA lowering intermediate: `.veee.vuma` (VEEE source → generated
  VUMA, preserves chain of custody for PMT verification)

---

## Part 2: Three-layer architecture formalization

This section formalizes the architecture from
`26-new-plans-three-layers.md` §"Architecture (browser-first)" as a
binding decision. Each layer is documented with owner, implementation
language, timeline, dependencies, and provides/consumes.

### Layer 1: VUMA — the systems language + compiler

| Property | Value |
|---|---|
| **Owner** | VUMA compiler team |
| **Implementation language** | Rust (compiler + codegen + runtime), Lean (proofs) |
| **Repository** | `workspace/vuma/` (existing repo — this audit's subject) |
| **Timeline** | Months 1–18 (per `26-new-plans-three-layers.md` layer table) |
| **Size** | ~5 kLOC Rust + Lean (per `26` bottom-line table); existing codebase is larger (parser + IR + 19 backends + 82 Lean files) |

**What VUMA provides to the layer above (WOMB)**:
- A compiled-code target: `.vuma` source → AST → SCG → IR → 19-arch
  native codegen, OR → `wasm32` for browser.
- PMT verification: Z3 contract discharge (`src/ive/src/verification.rs`
  `discharge_contracts_and_prove_blocks`), runtime `__oob_trap` bounds
  check (`src/codegen/src/memory_safety.rs` `inject_bounds_check_ir`),
  Lean `pmt_soundness` + Iris separation-logic layer (`proof/PMT/`).
- The PMT arena `___pmt_buffer` (`src/codegen/src/runtime/arena.rs`):
  type-agnostic flat byte slab, the substrate every WOMB allocation
  lives in.
- Capability model (`src/codegen/src/capability.rs`,
  `src/codegen/src/ipc.rs`): compile-time minted HMAC-SHA256 capability
  tokens (target state per ADR-0007; current FNV-1a × 4).
- Linear channels (`Resource::Channel` at `ipc.rs:521`, single-use
  windows at `:1222`) — session-typed IPC for IME etc.
- E-graph IR optimization (`src/codegen/src/egraph.rs`, 3235 lines;
  `bv_verify.rs`; `proof_artifacts.rs`) — shared infrastructure VEEE
  will reuse for its lowering.
- `extern "C"` host-import ABI: the only non-VUMA boundary, used by
  WOMB to call browser host imports (WebGL2 / WebGPU / EditContext /
  fetch / etc.).
- Z3 is a HARD dependency (`src/ive/Cargo.toml:17-22`: "The 'V' in VUMA
  depends on Z3") — the verification guarantee is non-negotiable.

**What VUMA consumes from the layer below (host)**:
- For browser target: a JS host shim (~500 LOC, no libraries) that
  owns the `<canvas>`, `EditContext`, `SharedArrayBuffer` IrqRing, and
  browser APIs (`fetch`, `WebSocket`, `ResizeObserver`). The shim is
  *not* a layer of the architecture — it is host code, like a libc.
- For native target (post-v1, v2): direct syscalls via VUMA's existing
  19 backends (no shim).

**What VUMA does NOT do**:
- Write UI code (that's WOMB).
- Define an ergonomic syntax (that's VEEE).
- Depend on external libraries in the *output binary* (compiler Rust
  crates are OK; runtime has zero deps).
- Touch the DOM directly (the JS host shim does that).

### Layer 2: WOMB — the UI engine libraries, written in VUMA

| Property | Value |
|---|---|
| **Owner** | UI engine team |
| **Implementation language** | VUMA (i.e., `.vuma` source files compiled by Layer 1) |
| **Repository** | `workspace/vuma/womb/` (existing tree; currently has `kernel/`, `crypto/`, `net/`, `string/`, `lib/` — UI subdirectory `womb/ui/` is new WOMB v1 work) |
| **Timeline** | Months 3–18 (starts after VUMA V-0 bridge fixes unblock f32 + nested structs; per `26` §"Layer 2") |
| **Size** | ~40 kLOC VUMA (per `26` bottom-line table) |

**What WOMB provides to the layer above (VEEE)**:
- A *machine-generatable* VUMA API surface: `LayoutNode`, `SceneNode`,
  `PathSegment`, `UiEvent`, `ImeState`, `SemanticsNode`, `Theme`, the
  event dispatcher, the animation system. VEEE's lowering target is
  *these APIs*, not raw VUMA IR.
- A reusable component vocabulary (Text, Button, Column, Row,
  ScrollView, TextField) that VEEE's standard library imports.
- The Flexbox / path-tessellation / font-shaping / BiDi / IME / a11y
  / clipboard / network-bridge implementations — all PMT-verified
  because they're written in VUMA.

**What WOMB consumes from the layer below (VUMA)**:
- VUMA's typed-state API (`State<T>`, `#[marshal]`, `#[borrow]`,
  `ArgMode::Borrow/Marshal/MayRetain/ForeignPass/Invalidate` at
  `src/codegen/src/marshal.rs:66-79`).
- VUMA's PMT arena for all allocations.
- VUMA's capability model for resource tokens (Canvas, GpuDevice,
  EditContext, Clipboard, Font).
- VUMA's `extern "C"` ABI to call browser host imports.
- VUMA's session-typed channels for the IME pipeline (V-11).

**What WOMB does NOT do**:
- Change the VUMA compiler (that's VUMA work).
- Define a new end-user syntax (that's VEEE).
- Link external C libraries (everything is VUMA or browser APIs via
  `extern "C"`).
- Contain the JS host shim (that's a separate ~500-LOC file).

**Critical WOMB-side dependency** (not in `26` but in `22` Part 4):
`womb/crypto/mac_kdf/hmac.vuma` already exists and is what ADR-0007 will
wire into VUMA's `ipc.rs:compute_signature` to replace the FNV-1a × 4
construction. WOMB provides the crypto; VUMA consumes it. This is the
concrete mechanism that makes the three-layer separation work — VUMA
doesn't *need* a crypto dependency because WOMB supplies it from VUMA
source.

### Layer 3: VEEE — the UX language + compiler

| Property | Value |
|---|---|
| **Owner** | VEEE language team (post-v1 hire; per `26` "Post-v1 (months 18-26): Vell team" — rename: VEEE team) |
| **Implementation language** | Rust (the VEEE compiler itself) |
| **Repository** | TBD — recommend a separate repo `veee` (sibling to `vuma`), NOT a subdirectory. Rationale: (a) the VEEE compiler is a separate Rust binary (`veeec`); (b) VEEE v0.1 ships 8 months after VUMA v1.0, so the release cadence is decoupled; (c) the VEEE team is a separate hire. The repo depends on `vuma` as a Cargo dependency for AST/SCG lowering. |
| **Timeline** | Months 18–26 (per `26` §"Layer 3"). Phase E-0 (language design) at month 18, E-1 (compiler) at month 20, E-2 (standard library) at month 24, v0.1 ship at month 26. |
| **Size** | ~15 kLOC Rust (per `26` bottom-line table) |

**What VEEE provides to the layer above (UI developers)**:
- An incremental-computation-first UX language: UI is a demand-driven
  query graph (Salsa), collections use seminaïve delta evaluation
  (Datafun), rendering uses algorithm/schedule separation (Halide).
- Ergonomic, structured-editing-friendly syntax (record-based styles,
  no CSS, no JSX). See Part 3.5.
- A type system with **signals** (incremental query sources),
  **monotonicity types** (delta-eligible collection operations), and
  session types (inherited from VUMA's V-11 work).
- Compile-time *verified* output: VEEE → VUMA AST, then VUMA's IVE
  runs PMT discharge on the generated code. The verification guarantee
  flows upward.

**What VEEE consumes from the layer below (WOMB + VUMA)**:
- **From WOMB**: the `womb/ui/` API surface (LayoutNode, SceneNode,
  UiEvent, etc.). VEEE's standard library wraps these.
- **From VUMA**: the AST data structures (`src/parser/src/ast.rs`)
  that VEEE lowers to; the e-graph infrastructure
  (`src/codegen/src/egraph.rs`) for VEEE→VUMA lowering optimization;
  the IVE pipeline for verification of the generated AST; the codegen
  for native/Wasm emission.
- VEEE does NOT re-implement verification, capability, or codegen. It
  reuses VUMA's stack end-to-end.

**What VEEE does NOT do**:
- Change VUMA's codegen or PMT (it compiles *to* VUMA AST; VUMA does
  the rest).
- Run at runtime (VEEE is compile-time only; the output is
  VUMA/Wasm).
- Have its own runtime or verification (it reuses VUMA's PMT,
  e-graph, capability model).
- Ship before VUMA v1 (VEEE depends on VUMA + WOMB being complete).

### Dependency graph (ASCII)

```
                ┌──────────────────────────────────────────────────────┐
                │  UI developer (writes .veee source)                  │
                └───────────────────────┬──────────────────────────────┘
                                        │  veeec (compile-time only)
                                        ▼
                ┌──────────────────────────────────────────────────────┐
                │  Layer 3: VEEE — Verified Expression Evaluation      │
                │  Engine                                              │
                │  ─ Owner:     VEEE language team                     │
                │  ─ Language:  Rust (the veeec compiler)              │
                │  ─ Timeline:  Months 18–26                          │
                │  ─ Provides:  .veee → VUMA AST lowering              │
                │  ─ Consumes:  WOMB ui/* API, VUMA AST + e-graph      │
                │  ─ Verified:  via VUMA IVE (PMT discharge)           │
                └───────────────────────┬──────────────────────────────┘
                                        │  lowers to VUMA AST
                                        ▼
                ┌──────────────────────────────────────────────────────┐
                │  Layer 2: WOMB — UI engine libraries                 │
                │  ─ Owner:     UI engine team                         │
                │  ─ Language:  VUMA (.vuma source)                    │
                │  ─ Timeline:  Months 3–18                           │
                │  ─ Provides:  LayoutNode, SceneNode, UiEvent, IME,   │
                │               a11y, fonts, shaper, bidi, renderer    │
                │  ─ Consumes:  VUMA State<T>, PMT arena, capability   │
                │               model, extern "C" host-import ABI      │
                │  ─ Verified:  inherits VUMA PMT (it IS VUMA code)    │
                └───────────────────────┬──────────────────────────────┘
                                        │  written in VUMA, compiled by
                                        ▼
                ┌──────────────────────────────────────────────────────┐
                │  Layer 1: VUMA — systems language + compiler         │
                │  ─ Owner:     VUMA compiler team                     │
                │  ─ Language:  Rust (compiler) + Lean (proofs)        │
                │  ─ Timeline:  Months 1–18                           │
                │  ─ Provides:  PMT verification (Z3 + __oob_trap +    │
                │               Lean), 19-arch codegen, wasm32 target, │
                │               e-graph IR, capability model, linear   │
                │               channels, ___pmt_buffer arena          │
                │  ─ Consumes:  host syscalls / browser host shim      │
                │  ─ Verified:  self (Lean proofs in proof/PMT/)       │
                └───────────────────────┬──────────────────────────────┘
                                        │  extern "C" host imports
                                        ▼
                ┌──────────────────────────────────────────────────────┐
                │  Host (NOT a layer of the architecture)              │
                │  ─ Browser: JS shim (~500 LOC) + <canvas> + EC + SAB │
                │  ─ Native (v2): Linux/Windows/macOS syscalls         │
                └──────────────────────────────────────────────────────┘

Cross-layer re-use (concrete, verified in source):
  • VEEE → VUMA e-graph:  src/codegen/src/egraph.rs (3235 lines, reused)
  • WOMB → VUMA capability: src/codegen/src/capability.rs (consumed)
  • VUMA → WOMB HMAC: womb/crypto/mac_kdf/hmac.vuma (consumed by VUMA's
                         ipc.rs:compute_signature per ADR-0007)
```

The last cross-layer arrow is notable: VUMA consumes WOMB's HMAC. This is
not a layering violation — it's the bootstrapping pattern. The capability
model is *compiled* by VUMA but its crypto is *implemented* in WOMB
(PMT-verified VUMA source). The compiler depends on its own output.

---

## Part 3: VEEE design space summary

This section summarizes the research from
`23-vell-redesign-research.md` and maps each borrowed idea to concrete
VUMA-side infrastructure.

### 3.1 Incremental computation (Salsa / Adapton)

**What VEEE borrows** (from `23-vell-redesign-research.md` §2.1):
- Queries memoized by inputs.
- Input→query dependency tracking: when an input changes, only queries
  that read it are invalidated.
- On-demand (lazy): queries only run when their result is read.
- Durable incrementality: results persist across runs (rust-analyzer
  pattern).

**Application to UI**: The UI is a function of state. When state
changes, only the affected subtrees should recompute — not the whole
tree (virtual DOM diffing) and not the whole frame (retained-mode
redraw).

**How this compiles to VUMA**:
- VEEE `signal count : i32 = 0` lowers to a VUMA `State<i32>` field in
  a generated `LayoutNode`-shaped struct.
- VEEE `ui counter_view = ... depends on count ...` lowers to a VUMA
  `transform` that reads `state.count` and produces a `SceneNode` delta.
- The "invalidation graph" becomes a compile-time-generated set of
  `layout_mark_dirty(offset)` calls in each state-write transform —
  VEEE's reactivity analyzer emits these automatically.
- The "demand-driven" property is a runtime concern of the VEEE
  *runtime shim* that links against WOMB's frame scheduler
  (`womb/ui/render/frame.vuma` per `26` Phase W-5). VEEE itself does
  not implement incremental computation at runtime — it generates VUMA
  code that *uses* WOMB's frame scheduler.

### 3.2 Monotonicity types (Datafun)

**What VEEE borrows** (from `23` §2.2):
- Sets are first-class; collection operations return sets.
- Monotonicity types: the type system tracks which operations are
  monotone (preserving set ordering), enabling **seminaïve
  evaluation** — only compute the *delta* of each set operation.
- Datalog-style fixpoint recursion with termination guarantees.

**Application to UI**: A list of items is a set. When you add an item,
you shouldn't re-render the whole list — you should compute the delta
(one new item) and render only that. Current UI frameworks do this
ad-hoc (React `key` props, Flutter diffing); Datafun makes it
principled.

**Interaction with VUMA's PMT arena**:
- A VEEE `signal items : Set<Item> = {}` lowers to a VUMA arena-
  allocated slab of `Item` structs plus a header (count, capacity,
  monotonic-version counter).
- Seminaïve delta evaluation compiles to: (a) a write transform that
  appends one `Item` and bumps the version; (b) a read transform that
  compares its cached version to the live version, and on mismatch
  re-iterates *only the new tail*.
- The monotonicity *type* is a VEEE-layer property — VUMA's IR has no
  notion of "monotone." VEEE's type checker rejects non-monotone
  operations in delta-eligible positions; the lowered VUMA code is
  just ordinary loads/stores. (See ADR-6 in Part 4.)
- The PMT arena is unaffected: monotonicity is a *type-system*
  guarantee, not a memory-layout guarantee. The arena holds bytes;
  VEEE's types constrain what byte-level operations the generated VUMA
  code performs.

### 3.3 Algorithm/schedule separation (Halide)

**What VEEE borrows** (from `23` §2.3):
- Separate the **algorithm** (what to compute: paths, colors,
  transforms) from the **schedule** (how to compute it: GPU passes,
  tessellation strategy, buffer layout).
- The compiler explores the schedule space (autotuning) and generates
  optimized code per target.

**Application to UI rendering**: A UI scene is an algorithm; the
rendering schedule (which GPU passes, what tessellation strategy, what
buffer layout) is a separate concern. Current UI frameworks bake the
schedule into the algorithm (you write `draw_quad()` calls). Halide-
style separation lets the compiler choose the optimal schedule per
platform.

**Mapping to VUMA's IR**:
- The VEEE `render app = ...` declaration lowers to a VUMA transform
  that builds a `SceneNode` tree (algorithm). This transform emits no
  GPU calls — it just constructs the scene description in the PMT
  arena.
- The VEEE `schedule { tessellation: gpu_compute, batching:
  per_material, ... }` block lowers to *compiler-chosen* WOMB
  renderer configuration: it sets fields on `womb/ui/render/scene.vuma`'s
  `RenderConfig` struct, which WOMB's renderer reads at frame time.
- VUMA's IR gains no new "schedule" concept. The schedule is a value
  (a struct of enum tags) that the renderer interprets. This is the
  cleanest mapping: VEEE's schedule DSL is sugar for a record
  constructor.
- For GPU autotuning: deferred to post-v0.1. The v0.1 schedule is
  static per build target. Autotuning (exploring the schedule space
  at build time) is a v0.2+ feature, listed in Part 4 as low-
  confidence for an ADR.

### 3.4 E-graph optimization (verified)

**What VEEE borrows** (from `23` §2.4):
- E-graphs represent all equivalent forms of an expression
  simultaneously. Equality saturation applies rewrite rules until no
  more apply, then extracts the best representation.
- VUMA already uses e-graphs — the VEEE→VUMA lowering should reuse
  the same infrastructure.

**Verification (K-1, against actual source)**:
- ✓ `src/codegen/src/egraph.rs` exists. 3235 lines.
  - `pub enum ENode` at line 81
  - `pub struct EGraph` at line 141
  - `pub fn new()` at 165, `pub fn add()` at 176, `pub fn find()` at
    193, `pub fn merge()` at 206, `pub fn rebuild()` at 250
  - File-level docstring (lines 1–60) documents: congruence-closure
    rebuild, bottom-up DP extraction, commutativity/associativity/
    distributivity rules, **PMT state-operation ENodes** including
    `StateInit`, `StateRead`, `StateWrite`, `StateTransform`, plus
    rewrite rules `state_dead_init_elim`, `state_store_load_forward`.
    This is the *exact* infrastructure VEEE needs for lowering UI
    reactivity (the store-load-forward rule is what makes "the
    counter_view recompute after count changes" cheap).
- ✓ `src/codegen/src/bv_verify.rs` exists (bitvector verification —
  proof-carrying e-graph rewrite rules).
- ✓ `src/codegen/src/proof_artifacts.rs` exists (proof artifact
  emission for verified rewrites).
- ✓ `tests/egraph_extraction_tests.rs` exists (test coverage).

**Implication for VEEE**: VEEE→VUMA lowering does not need to invent a
new optimizer. It needs to emit VUMA AST nodes that the existing
egraph pass can saturate. The store-load-forward rule alone handles
most "signal changed → recompute dependent view" patterns for free.

### 3.5 Structured-editing-friendly syntax

**What VEEE borrows** (from `23` §2.5):
- Hazel-style "live" editing: the editor always shows a valid AST;
  holes are first-class; no parser errors block the editor.
- Projectional editing: the user edits the AST directly; the text
  representation is a projection.

**What the syntax looks like** (from `23` §3 revised Vell syntax,
adapted to VEEE):

```veee
-- VEEE: Verified Expression Evaluation Engine
-- No HTML clone. No CSS. No imperative event handlers.

-- State is a signal (Salsa-style incremental query)
signal count : i32 = 0
signal items : Set<Item> = {}        -- Datafun-style set

-- UI is a pure function of signals; the compiler tracks dependencies
ui counter_view =
  column [spacing 8] [
    text (show count) [font_size 16, color #333],
    button "Increment"
      [bg #06C, color white, radius 4, padding (8, 16)]
      (on_click => count := count + 1),
    button "Reset"
      [bg #C00, color white, radius 4, padding (8, 16)]
      (on_click => count := 0)
      (disabled => count == 0)
  ]

-- Collections use Datafun-style seminaïve delta computation
ui item_list =
  column [] (map items (\item ->
    row [spacing 8] [
      text item.name [],
      button "Delete" (on_click => items := items \ {item})
    ]
  ))

-- Algorithm/schedule separation (Halide-inspired)
render app =
  window "Counter" (400, 300) [
    counter_view,
    item_list
  ]

schedule {
  tessellation: gpu_compute,
  batching: per_material,
  scroll: composited_layer,
  text: path_per_glyph
}
```

**Properties that make this structured-editing-friendly**:
1. **Record-based styles** (`[bg #06C, radius 4]`) — every style is a
   flat record, parseable as an AST node with named children. No
   cascade, no specificity, no parser ambiguity.
2. **Pure state transitions** (`(on_click => count := count + 1)`) —
   no imperative block, no statement list. Every handler is a single
   assignment expression.
3. **No nested precedence** — function application is the only
   binding form; no operator-precedence parser needed.
4. **Holes are syntactically valid** — `_` in any expression
   position is a typed hole (Hazel pattern). The editor can always
   render a valid AST.

### 3.6 Backend strategy (Cranelift + MLIR→SPIR-V + VUMA codegen)

**The three-option decision** (from `23` §4):

| Option | Role | Pros | Cons | Verdict |
|---|---|---|---|---|
| **Cranelift** | Dev builds | Rust-native, e-graph-based (ISLE), fast compiles, used by Wasmtime | 4-arch coverage (x86_64, aarch64, riscv64, s390x), optimization slightly below LLVM | **Adopt for dev builds** |
| **MLIR** | Mid-level IR + GPU path | Multi-level dialects (`vell` → `affine` → `memref` → `spirv` for GPU), SPIR-V dialect gives GPU support without custom backend, eqsat dialect (2025) brings e-graphs to MLIR, industry-standard (TF, JAX, IREE, Flang) | C++ (LLVM ecosystem), heavyweight, learning curve | **Adopt for GPU path** |
| **LLVM directly** | (rejected) | Best optimization passes, 19-arch coverage | C++, ~50 MB, no e-graphs, no GPU dialect (need MLIR or custom GPU backend anyway) | **Reject** — throws away VUMA's e-graph infrastructure |

**Recommended backend strategy** (from `23` §4):
```
VEEE (.veee source)
  ↓ (veeec: e-graph-based lowering, reuses VUMA's egraph.rs)
VUMA AST (PMT-verified by IVE)
  ↓ (VUMA codegen)
VUMA IR (e-graph optimized)
  ↓
  ├─ Dev builds:    Cranelift (fast compiles, e-graph, Rust)
  ├─ GPU:           MLIR (vell dialect → affine → spirv)
  └─ Production:    VUMA's custom 19-arch codegen (PMT-checked, verified)
```

**Effort** (from `23` §4):
- Cranelift backend: ~2 months (Rust crate; integration straightforward).
- MLIR GPU pipeline: ~4 months (define VUMA→MLIR lowering, SPIR-V
  dialect emission).
- Keep VUMA's custom codegen: 0 (already exists, 19 arches).

**Why NOT LLVM directly**:
1. **LLVM is C++** — VUMA is Rust. Linking LLVM adds a heavy C++
   dependency.
2. **LLVM doesn't do e-graphs** — VUMA already uses e-graphs
   (`egraph.rs`, `bv_verify.rs`); LLVM's passes are hand-written
   peephole+pattern-match. The modern research direction (Cranelift,
   egglog, MLIR eqsat) is e-graph-based.
3. **LLVM is huge** (~50 MB) and slow to compile — bad for fast dev
   iteration.
4. **LLVM's best production optimization passes** are less important
   than PMT's formal guarantees — VUMA's value proposition is
   verification, not raw code speed.
5. **LLVM has no GPU dialect** — you'd need MLIR or a custom GPU
   backend anyway. If you're adopting MLIR for GPU, you get the
   mid-level IR benefits for free; adding LLVM on top is redundant.

---

## Part 4: Architectural decisions needing ADRs

Each decision below is assessed for confidence. **High-confidence**
decisions get ADRs in Wave L (the next ADR wave, after Wave F's
ADR-0011). **Low-confidence** decisions get deferred — they need more
design work or a prototype before they can be locked in.

The next free ADR number is **ADR-0012** (existing series ends at
ADR-0011, the Wave F meta-ADR).

### ADR-0012: Adopt VEEE as the name for the UX language layer (renames VELL)

| Field | Value |
|---|---|
| **Topic** | Naming the UX language layer |
| **Options** | (A) Keep "VELL"; (B) Rename to "VEEE" (Verified Expression Evaluation Engine); (C) Rename to something else |
| **Recommendation** | Option B — VEEE |
| **Confidence** | **High** |
| **Wave L ADR?** | Yes — ADR-0012 |

**Rationale**: See Part 1. The rename is a branding decision with
zero technical risk; the SWE package's technical content is unaffected.
Locking the name now (months before VEEE v0.1 design starts at month
18) means all subsequent design docs use the new name.

**Consequences**:
- Positive: distinctive brand, explicit VE^3 reference, no clash with
  Vellum/Vela.
- Negative: every doc that says "VELL" needs a sed pass. The SWE
  package files (22, 23, 26) stay as historical artifacts; new docs
  use VEEE.
- Neutral: the `.vell` → `.veee` file extension change is mechanical.

### ADR-0013: Adopt the three-layer architecture (VUMA / WOMB / VEEE)

| Field | Value |
|---|---|
| **Topic** | Formalize the three-layer separation |
| **Options** | (A) Keep VUMA + WOMB (no UX language); (B) Three layers VUMA / WOMB / VEEE as documented; (C) Two layers VUMA + VEEE (no WOMB — VEEE lowers directly to VUMA without an intermediate engine) |
| **Recommendation** | Option B |
| **Confidence** | **High** |
| **Wave L ADR?** | Yes — ADR-0013 |

**Rationale**: See Part 2. The three-layer separation is what makes
the timeline viable: VUMA v1 (month 18) ships a *usable* browser UI
engine written in VUMA; VEEE v0.1 (month 26) makes it ergonomic.
Without WOMB, VEEE would have to lower directly to VUMA IR and
re-implement layout/rendering/text in its own runtime — at least
12 months of extra work.

**Why not Option C (collapse WOMB into VEEE)**:
- WOMB is VUMA code, PMT-verified. If WOMB were VEEE code, it would
  lose PMT verification (VEEE has no runtime verification of its
  own).
- WOMB v1 ships at month 18; VEEE v0.1 ships at month 26. WOMB is
  usable *before* VEEE exists — early adopters write VUMA directly.
- The WOMB API is the *compilation target* VEEE lowers to. Without
  WOMB, VEEE's lowering target is raw VUMA IR (much harder to
  generate correctly).

**Consequences**:
- Positive: each layer has a clear owner, language, and timeline.
  VUMA stays minimal; WOMB stays in VUMA; VEEE stays compile-time.
- Negative: three teams, three repos (or three sub-trees), three
  release cadences.
- Neutral: the JS host shim is *not* a layer — it's host code, like
  a libc.

### ADR-0014: VEEE compiles to VUMA AST, not to VUMA IR

| Field | Value |
|---|---|
| **Topic** | VEEE's lowering target |
| **Options** | (A) VEEE → VUMA AST (then VUMA pipeline runs end-to-end: AST → SCG → IR → codegen); (B) VEEE → VUMA SCG (skip AST); (C) VEEE → VUMA IR directly (skip AST + SCG) |
| **Recommendation** | Option A — VEEE → VUMA AST |
| **Confidence** | **High** |
| **Wave L ADR?** | Yes — ADR-0014 |

**Rationale**: VEEE's value proposition is *verified* UI. If VEEE
lowers to VUMA IR directly, it bypasses the IVE (which runs on the
SCG) and the contract discharge pass — VEEE programs would be
unverified. Lowering to AST means VEEE output gets the full PMT
pipeline: Z3 contract discharge, `__oob_trap` runtime bounds check,
Lean `pmt_soundness` theorem.

**Why not Option B (VEEE → SCG)**: the SCG is an intermediate data
structure produced by `parser::to_scg::AstToScg` (semantic SCG) or
`pipeline::bridge_ast_to_codegen_scg_with_meta` (codegen SCG). Both
expect an AST input; jumping straight to SCG means duplicating the
AST→SCG bridge logic in the VEEE compiler. Worse, the two-SCG
architecture (`vuma-side-research-draft-v2.md` §1.1) means VEEE
would have to produce *both* SCGs and ensure they agree — exactly
the parity problem that V-03 / V-NEW-2 just fixed. Lowering to AST
sidesteps this entirely.

**Why not Option C (VEEE → IR)**: bypasses all verification. Reject
outright.

**Consequences**:
- Positive: VEEE programs inherit full PMT verification. VEEE's
  compiler is simpler (only needs to emit AST nodes).
- Negative: VEEE is coupled to the VUMA AST shape. If VUMA's AST
  changes, VEEE's lowering must change. Mitigation: the AST is a
  documented compilation target (`22` Part 5 "Document the VUMA ABI
  as a compilation target").
- Neutral: VEEE's compile time includes the full VUMA pipeline
  (AST → SCG → IR → codegen). Acceptable — VEEE is not a hot-reload
  language at v0.1.

### ADR-0015: VEEE uses Cranelift for dev builds, VUMA codegen for production

| Field | Value |
|---|---|
| **Topic** | VEEE's backend strategy |
| **Options** | (A) Cranelift (dev) + VUMA codegen (prod) + MLIR/SPIR-V (GPU); (B) LLVM only; (C) VUMA codegen only (no Cranelift, no MLIR) |
| **Recommendation** | Option A |
| **Confidence** | **Medium** |
| **Wave L ADR?** | Yes — ADR-0015, but flagged as "needs prototype validation" |

**Rationale**: See Part 3.6. The Cranelift+MLIR+VUMA-codegen
combination aligns with VUMA's existing e-graph infrastructure and
the Rust codebase. Cranelift gives fast dev iteration; MLIR gives a
GPU path without a custom backend; VUMA codegen gives verified
production builds.

**Why Medium confidence, not High**:
- Cranelift's 4-arch coverage (x86_64, aarch64, riscv64, s390x) is
  sufficient for dev builds but means dev builds can't target the
  other 15 arches. For a UI language, this is fine (dev is on the
  developer's laptop; prod is wasm32 or one of the 4 Cranelift
  arches). But the workflow needs to be documented.
- MLIR is C++, which violates VUMA's small-deps policy (ADR-0010
  mandates ≤5 external Rust crates; MLIR is not a Rust crate).
  Mitigation: MLIR is a *build-time* dependency (the compiler links
  it; the output binary doesn't). The small-deps policy applies to
  the *output binary*, not the compiler. But this needs to be
  documented in the ADR.
- The 2-month Cranelift + 4-month MLIR estimates from `23` §4 are
  rough; they need prototype validation before being locked in.

**Why not Option B (LLVM only)**: see Part 3.6 — throws away e-graph
infrastructure, C++ dep, no GPU dialect.

**Why not Option C (VUMA codegen only)**: misses the GPU path
entirely. VUMA's 19-arch codegen is CPU-only (verified:
`backend.rs:784` BackendKind has 19 variants, all CPU ISAs — no
Vulkan/Metal/WebGPU variant, per `22` Part 1 Claim B REFUTED). For
a UI language, GPU is essential (vector renderer, path tessellation
compute shader per `26` Phase W-5).

**Consequences**:
- Positive: dev iteration is fast (Cranelift); GPU support exists
  (MLIR/SPIR-V); production is verified (VUMA codegen).
- Negative: three backends to maintain. MLIR is a C++ build-time
  dependency (acceptable per small-deps policy, but documented).
- Neutral: the VEEE compiler emits VUMA AST; the choice of backend
  is a VUMA-level concern, not a VEEE-level concern. VEEE doesn't
  know which backend is in use.

### ADR-0016: VEEE's incremental computation engine lives in VEEE, not VUMA

| Field | Value |
|---|---|
| **Topic** | Where the incremental-computation runtime lives |
| **Options** | (A) In VEEE (the veeec compiler generates VUMA code that implements the query/invalidation graph); (B) In VUMA (add `IRInstr::Signal`, `IRInstr::Query`, `IRInstr::Invalidate` to VUMA's IR); (C) In WOMB (a `womb/ui/reactive.vuma` library that VEEE generates calls to) |
| **Recommendation** | Option A (with a thin Option C helper) |
| **Confidence** | **High** |
| **Wave L ADR?** | Yes — ADR-0016 |

**Rationale**: VUMA should stay minimal. Adding incremental-computation
primitives to VUMA's IR pollutes the systems language with UI-specific
concepts. The right split: VEEE's compiler generates ordinary VUMA
code (loads, stores, transforms, `layout_mark_dirty` calls) that
*implements* the query/invalidation graph. VUMA's IR stays
domain-neutral.

**Concrete lowering**:
- VEEE `signal count : i32 = 0` → VUMA `state count: i32 = 0` (a
  field in a generated state struct).
- VEEE `ui counter_view = ... depends on count ...` → a VUMA
  `transform counter_view_recompute(state: State<...>)` that reads
  `state.count` and writes to the scene tree. Plus a per-frame check:
  `if state.count_version != cached_count_version { recompute }`.
- The "invalidation graph" is a compile-time-computed static
  dependency map (VEEE's reactivity analyzer emits it). At runtime
  it's just a switch on which signals changed since last frame.
- Optional WOMB helper (`womb/ui/reactive.vuma`): a small library
  that VEEE-generated code can call for the version-comparison
  boilerplate. This keeps the VEEE compiler simpler. But the
  *logic* is in VEEE-generated VUMA code; WOMB just provides
  utility functions.

**Why not Option B (VUMA IR primitives)**:
- Pollutes VUMA with UI-specific IR instructions.
- Requires Lean proofs for the new instructions (significant effort,
  per the V-14 lesson — adding f32 to PMT is 2-4 weeks just for the
  bit-pattern model).
- The IVE would need to learn about query/invalidation semantics.
- No benefit: the same code can be generated as ordinary VUMA
  loads/stores.

**Why not Option C alone (WOMB library only)**:
- VEEE's compile-time analysis (dependency graph, invalidation
  schedule) is the *value* of the language. Pushing it to a runtime
  WOMB library loses the static guarantees.
- But a thin WOMB helper for runtime utilities (version counters,
  dirty-bit management) is fine. Option A includes this.

**Consequences**:
- Positive: VUMA stays minimal. VEEE owns its reactivity model.
  Lean proofs are unaffected (no new IR instructions).
- Negative: VEEE-generated VUMA code is verbose (manual
  invalidation calls). Mitigation: the e-graph optimizer
  (`egraph.rs` with `state_store_load_forward` rule) simplifies
  redundant invalidations automatically.
- Neutral: VEEE's reactivity analyzer is a non-trivial compiler
  pass (~4 weeks per `26` Phase E-1 "Reactivity analyzer").

### ADR-0017: VEEE's monotonicity types are a VEEE-layer type-system feature, not a VUMA IR feature

| Field | Value |
|---|---|
| **Topic** | Where monotonicity types live |
| **Options** | (A) VEEE-layer type system (VEEE type-checks, lowers to ordinary VUMA code); (B) VUMA IR type system (add monotonicity annotations to VUMA types); (C) No monotonicity types (use ordinary collections, accept full re-renders) |
| **Recommendation** | Option A |
| **Confidence** | **High** |
| **Wave L ADR?** | Yes — ADR-0017 |

**Rationale**: Same principle as ADR-0016. Monotonicity is a type-
system property that constrains *what operations are allowed in
delta-eligible positions*. It compiles to ordinary VUMA code: the
type checker rejects non-monotone operations; the lowered VUMA code
is just loads/stores/loops. VUMA's IR has no notion of "monotone" —
and it shouldn't.

**Concrete lowering**:
- VEEE `signal items : Set<Item> = {}` → VUMA state with a
  versioned slab (count, capacity, version counter, items array).
- VEEE `map items (\item -> ...)` with monotonicity proof →
  VUMA code that iterates the slab and applies the function. The
  seminaïve delta is a *compile-time* optimization: VEEE emits
  *two* transforms — one for the initial fill, one for the delta
  (only new items).
- The monotonicity *type* ensures the delta transform is sound
  (the function is monotone, so applying it to the delta is
  equivalent to applying it to the full set and diffing).

**Why not Option B (VUMA IR types)**:
- VUMA's IR is untyped at the type-system level (it's a sequence
  of `IRInstruction` values; types are `IRType` enum variants
  like `I32`, `F64`, not qualified types). Adding monotonicity
  would require a type system on top of the IR — a major
  architectural change.
- Same Lean-proof burden as ADR-0016 Option B.

**Why not Option C (no monotonicity types)**:
- Loses the principled basis for delta evaluation. VEEE would
  either (a) re-render whole lists on any change (React without
  keys), or (b) use ad-hoc heuristics (React keys). Both are
  worse than the Datafun approach.

**Consequences**:
- Positive: VUMA IR stays simple. VEEE's type system is the
  sole locus of monotonicity reasoning.
- Negative: VEEE's type checker is non-trivial (~4 weeks per
  `26` Phase E-1 "Type checker (signals, monotonicity, effects)").
- Neutral: the seminaïve delta evaluation is a VEEE compiler
  optimization, not a VUMA IR feature.

### ADR-0018: The GPU path for VEEE goes through MLIR→SPIR-V, not through VUMA's codegen

| Field | Value |
|---|---|
| **Topic** | GPU compilation path |
| **Options** | (A) VEEE → VUMA IR → MLIR (`vell`/`vuma` dialect → `affine` → `memref` → `spirv`); (B) Add a GPU backend to VUMA's 19-arch codegen (Vulkan/Metal/SPIR-V emitter); (C) GPU shaders written by hand in GLSL, embedded as const byte arrays (V-26) |
| **Recommendation** | Option A (with Option C as a transitional fallback for v0.1) |
| **Confidence** | **Medium** |
| **Wave L ADR?** | Yes — ADR-0018, but flagged as "needs MLIR prototype" |

**Rationale**: VUMA's codegen is CPU-only (verified: `backend.rs:784`
BackendKind has 19 variants, all CPU ISAs — per `22` Part 1 Claim B
REFUTED, "zero GPU support"). Building a custom GPU backend is 3-6
months (`22` Part 4 revised estimate). MLIR's SPIR-V dialect provides
GPU shader emission for ~4 months of work (`23` §4), leveraging
industry-standard infrastructure (TensorFlow, JAX, IREE, Flang all
use MLIR).

**Why Medium confidence**:
- MLIR is C++ — a significant build-time dependency. The VUMA
  codebase is pure Rust; introducing a C++ dependency is a
  cultural shift that needs team buy-in.
- The VUMA IR → MLIR dialect lowering is not yet designed. The
  "vell dialect" mentioned in `23` is a sketch, not a spec.
- The MLIR eqsat dialect (2025) is research-stage; production
  readiness unclear.
- The 4-month estimate is rough.

**Why transitional Option C for v0.1**:
- WOMB Phase W-5 (`26`) already plans hand-written SPIR-V shaders
  for path tessellation (`shaders/path_tessellate.comp.glsl` →
  `.spv`) and WebGL2 fallback rasterization
  (`shaders/path_rasterize.frag.glsl` → `.spv`), embedded via V-26
  (const byte arrays). This is *sufficient for v0.1* — the
  renderer has a fixed shader set, no runtime shader compilation.
- The MLIR path is for *v0.2+*, when VEEE wants to generate custom
  shaders from `schedule { tessellation: gpu_compute, ... }`
  declarations.

**Why not Option B (custom VUMA GPU backend)**:
- 3-6 months of work for a single GPU vendor (Vulkan OR Metal OR
  WebGPU). MLIR/SPIR-V gives all three for ~4 months.
- Maintaining a custom GPU backend duplicates work that MLIR
  already does.
- Throws away the industry-standard MLIR ecosystem (debuggers,
  profilers, dialect libraries).

**Consequences**:
- Positive: GPU support via industry-standard infrastructure.
  VEEE's `schedule` DSL can lower to MLIR dialects.
- Negative: MLIR is a C++ build-time dependency. The MLIR dialect
  design is non-trivial.
- Neutral: for v0.1, hand-written SPIR-V (Option C) suffices.
  ADR-0018 governs the v0.2+ path.

---

## Confidence assessment — Wave L ADR prioritization

| ADR | Topic | Confidence | Wave L? | Notes |
|---|---|---|---|---|
| ADR-0012 | VEEE name adoption | **High** | Yes | Branding decision, zero technical risk. Lock now. |
| ADR-0013 | Three-layer architecture | **High** | Yes | Architectural decision, well-motivated by timeline. |
| ADR-0014 | VEEE → VUMA AST (not IR) | **High** | Yes | Verification guarantee depends on this. Non-negotiable. |
| ADR-0015 | Cranelift dev / VUMA codegen prod / MLIR GPU | **Medium** | Yes (with prototype flag) | Cranelift integration is well-understood; MLIR needs a prototype before locking the 4-month estimate. |
| ADR-0016 | Incremental computation in VEEE | **High** | Yes | VUMA-stays-minimal principle. Strong precedent (Salsa is a library, not a language feature). |
| ADR-0017 | Monotonicity types in VEEE | **High** | Yes | Same principle as ADR-0016. Datafun's type system is well-understood. |
| ADR-0018 | GPU path via MLIR→SPIR-V | **Medium** | Yes (with prototype flag) | Needs MLIR dialect design + prototype before locking. Transitional V-26 hand-written SPIR-V path for v0.1. |

**Summary**: All 7 ADRs are Wave L candidates. **5 are High-confidence**
(ADR-0012, -0013, -0014, -0016, -0017) and should be written as
**Accepted** in Wave L. **2 are Medium-confidence** (ADR-0015, -0018)
and should be written as **Proposed** in Wave L, with an explicit
"prototype validation required" gate before promotion to Accepted.

**Deferred to post-prototype** (not Wave L):
- GPU schedule autotuning (Halide-style schedule-space exploration) —
  needs a working MLIR prototype before any ADR can lock the
  autotuning strategy.
- VEEE structured-editor design (Hazel-style projectional editing) —
  Phase E-3 in `26`, month 25-28, post-v0.1. No ADR needed until
  the editor prototype exists.
- VEEE LSP / hot reload / DevTools — Phase E-3, post-v0.1.
- VEEE v0.2+ features (concurrent signals, distributed UI,
  multi-window) — too early to design.

---

## References

### SWE package files (the research ground)
- `workspace/vuma-swe-package-3/vuma-swe-package/22-review-against-vuma.md` — Part 5 (Vell → VUMA architecture introduction), Part 4 (vector engine + GPU gap), Part 7 (Vell recommendation)
- `workspace/vuma-swe-package-3/vuma-swe-package/23-vell-redesign-research.md` — §2 (modern research survey: Salsa, Datafun, Halide, e-graphs, structured editing, MLIR), §3 (revised Vell syntax), §4 (Cranelift + MLIR backend strategy), §5 (revised three-layer architecture)
- `workspace/vuma-swe-package-3/vuma-swe-package/26-new-plans-three-layers.md` — Layer table (VUMA/WOMB/VELL timelines), Architecture (browser-first), Phase V-* / W-* / E-* plans, dependency graph, team allocation

### VUMA-side source (verified by K-1)
- `src/codegen/src/egraph.rs` — 3235 lines, `pub enum ENode` :81, `pub struct EGraph` :141, PMT state-operation ENodes + `state_store_load_forward` rewrite rule documented at file head
- `src/codegen/src/bv_verify.rs` — bitvector verification (proof-carrying rewrites)
- `src/codegen/src/proof_artifacts.rs` — proof artifact emission
- `src/codegen/src/backend.rs:784` — `pub enum BackendKind` with 19 variants (all CPU ISAs; no GPU)
- `src/codegen/src/capability.rs:117` — hardcoded `b"vuma_dev_signing_key"` (V-A3-2); `:17-45` docstring admits FNV-1a × 4 (not HMAC-SHA256)
- `src/codegen/src/ipc.rs:521` — `Resource::Channel(u64)`; `:1222` linear single-use windows
- `src/codegen/src/marshal.rs:66-79` — `ArgMode::Borrow/Marshal/MayRetain/ForeignPass/Invalidate`
- `src/codegen/src/runtime/arena.rs:68` — `pub struct Arena` (the PMT arena `___pmt_buffer`)
- `src/ive/Cargo.toml:17-22` — Z3 is a HARD dependency: "The 'V' in VUMA depends on Z3"
- `src/ive/src/verification.rs` — `discharge_contracts_and_prove_blocks` (Z3 contract discharge); `:2379` `let known = true` stub (V-A3-6, capability verifier)
- `womb/crypto/mac_kdf/hmac.vuma` — WOMB-side HMAC-SHA256 implementation (what ADR-0007 wires into VUMA's `ipc.rs:compute_signature`)
- `proof/PMT/` — 82 Lean files including `Iris/` (separation logic), `BitVecArena.lean`, `PipelineSim.lean`, `WellTypedStrong.lean`, `RawArena.lean`, 22-file `Faithful/` directory
- `docs/adr/` — ADR-0001 through ADR-0011 (existing); next free number is **ADR-0012**

### VUMA-side research draft
- `workspace/vuma/docs/vuma-side-research-draft-v2.md` — Wave F corrected draft. §1.1 (two-SCG architecture), §1.2 (`__oob_trap` defense in depth), §1.3 (Lean proof layer scope), §1.4 (capability model compile-time-only design). WOMB and VELL (now VEEE) explicitly out of scope — this K-1 report is the first formal introduction of WOMB and VEEE into the ADR series.

### External research cited (from `23` §7)
- Salsa: https://github.com/salsa-rs/salsa
- Adapton: http://adapton.org/
- Datafun: https://www.rntz.net/files/datafun.pdf
- Seminaïve Datafun: https://www.cl.cam.ac.uk/~nk480/seminaïve-datafun.pdf
- Halide: https://halide-lang.org/
- egg (Rust e-graph library): https://github.com/egraphs-good/egg
- Cranelift: https://github.com/bytecodealliance/cranelift
- MLIR: https://mlir.llvm.org/
- MLIR SPIR-V dialect: https://mlir.llvm.org/docs/Dialects/SPIR-V/
- Hazel (live programming): https://hazel.org/
- Laminar (FRP without virtual DOM): https://laminar.dev/virtual-dom
