# VEEE Layer — Fine Draft (Final Engineering Plan)

**Status**: Final draft.
**Scope**: Layer 3 (VEEE UX language + compiler, compiles to VUMA AST).
**Date**: 2026-08-01.
**Name**: VEEE = V·E·E·E = Verified Expression Evaluation Engine (per ADR-0012).
**Author**: Subagent BD-1 (VEEE-layer fine draft).

---

## 1. Executive summary

VEEE is the **Layer 3 UX language** of the three-layer VUMA architecture
(ADR-0013). It is a higher-level language that compiles to VUMA AST
(ADR-0014), from which point VUMA's full pipeline (parse → SCG → IVE → IR →
e-graph → regalloc → 19-backend codegen) takes over. The defining claim of
VEEE is the same claim that defines VUMA itself, lifted one layer:

> **Every program written in VEEE is verified by VUMA's PMT pipeline, with no
> separate verifier required in VEEE.**

This is the moat. React, SwiftUI, Jetpack Compose, Flutter, and the entire
HTML/CSS/JS stack ship *unverified* UI. VEEE programs lower to VUMA AST, run
through Z3 contract discharge, the `__oob_trap` runtime bounds check, the
Lean `pmt_soundness` theorem, and the 19-backend codegen — exactly the same
pipeline as hand-written VUMA. The user gets ergonomic UX-language syntax
*and* the formal memory-safety guarantee. Neither has to be sacrificed for
the other.

VEEE is **not** an HTML/CSS clone and **not** a JSX-with-extra-steps
clone. The SWE package's original "VELL" sketch (file `23-vell-redesign-research.md`
§1) explicitly repudiates that path. Instead, VEEE is built on four pillars
of modern programming-languages research, each grounded in concrete VUMA-side
infrastructure that already exists today (verified by K-1):

1. **Incremental computation** (Salsa / Adapton) — the UI is a demand-driven
   query graph, not a virtual DOM. A `signal` primitive lowers to VUMA's
   `State<T>` typed-state API; the invalidation graph lowers to a
   compile-time-generated static array plus `mark_dirty` calls into WOMB's
   `womb/ui/reactive/graph.vuma`. (ADR-0016.)
2. **Monotonicity types** (Datafun) — collection types carry a monotonicity
   qualifier (`monotone` or `antitone`) that the VEEE type checker enforces
   at compile time. This enables **seminaïve delta evaluation**: when a
   monotone set grows, the dependent views recompute only on the new
   elements, not the whole collection. The monotonicity property lowers to
   VUMA `requires`/`ensures` contracts that Z3 discharges. (ADR-0017.)
3. **Algorithm/schedule separation** (Halide) — a `render` block declares
   *what* to draw (a scene tree); a `schedule` block declares *when/how* to
   draw it (rendering strategy as a record value WOMB's renderer
   interprets). The schedule is a value, not a new IR concept. (K-1 §3.3.)
4. **E-graph optimization** — VEEE → VUMA AST → VUMA's existing e-graph
   (`src/codegen/src/egraph.rs`, 3235 lines), with VEEE-specific rewrite
   rules added alongside VUMA's existing PMT-state rewrites (notably
   `state_store_load_forward`, which is *exactly* what makes
   signal-change-driven reactivity cheap). No new optimizer. (K-1 §3.4.)

**Backend strategy** (ADR-0023, ADR-0022 — both Accepted, both supersede
the original Proposed ADR-0015 / ADR-0018):

- **Dev builds**: VUMA codegen with `--dev` flags (`opt_level=None`,
  `lto=false`, `codegen_units=16`, host ISA only, verification still ON).
  Measured baseline: VUMA compiles `tests/gold_standard/float_advanced/fp_bench.vuma`
  in ~1.3s with `opt_level=None`. No Cranelift. No new Rust crates.
- **Production builds**: VUMA's 19-backend codegen with the full
  optimization suite (e-graph rewriting, loop unrolling, vectorization,
  PGO, LTO, cross-compilation).
- **GPU shaders**: hand-written GLSL → `glslangValidator` (BSD-3,
  build-time only) → SPIR-V → embed as a const byte array via
  `#[embed("file.spv")]` (requires V-26: const byte arrays — small
  VUMA-side patch, deferred) → WOMB's `gpu_dispatch` host import. No MLIR.
  No LLVM. No Rust GPU crates.

All three tracks are consistent with VUMA's hand-write-everything
philosophy (hand-written lexer NFA, hand-written TOML parser, hand-written
HMAC-SHA256, hand-written 19 backends, hand-written e-graph). The 5-crate
external-dependency policy (ADR-0010) is preserved: VEEE adds **zero** new
external Rust crates. The only new build-time tool is `glslangValidator`
(which is a tool, not a crate, exactly like Z3).

**Timeline**: VEEE ships after VUMA v1 (month 18) and WOMB v1 (month 18,
parallel with VUMA). VEEE compiler skeleton at month 20, incremental
computation + monotonicity types at month 24, standard library + e-graph
rewrite rules + GPU path at month 26. **VEEE v0.1 ships at month 26.**
(See §9.)

**Estimated size**: ~5,000 lines of Rust for the VEEE compiler (`veeec`),
plus ~2,000 lines of VEEE standard library (which lowers to WOMB calls).
The ~5,000-line estimate is from ADR-0014: VEEE avoids duplicating VUMA's
type checker, effect inferencer, and verifier (which would have cost
~15,000 lines if VEEE lowered to IR directly). The compiler is small
because the lowering target is small.

This document is the **final engineering plan**. It locks the design
decisions that ADR-0012 through ADR-0017 and ADR-0022 through ADR-0023
have already accepted, fills in the language design with concrete syntax
and lowering rules, sketches five example programs (counter, todo list,
text label, animation, GPU shader dispatch), and lists the open questions
that remain for VEEE v0.2+.

---

## 2. VEEE identity (from ADR-0012)

| Property | Value |
|---|---|
| **Name** | VEEE |
| **Expansion** | V·E·E·E = **V**erified **E**xpression **E**valuation **E**ngine |
| **V** | VUMA — the systems language VEEE compiles to |
| **E₁** | **Verified** — VEEE programs inherit VUMA's PMT verification (Z3 contract discharge, `__oob_trap` runtime bounds check, Lean `pmt_soundness` theorem). The durable moat vs. React/SwiftUI/Compose. |
| **E₂** | **Expression** — VEEE is expression-oriented. UI is a pure expression of state, not a sequence of imperative build-tree-then-mark-dirty calls. |
| **E₃** | **Evaluation** — VEEE's defining mechanism is demand-driven incremental evaluation (Salsa) + seminaïve delta evaluation (Datafun). The "Evaluation" word anchors the brand on this distinguishing feature. |
| **Pronunciation** | "vee-eee" (three syllables, primary) or "vee-cubed" (casual) |
| **Logo** | `V³` (V with superscript 3) — works at favicon size |
| **File extension** | `.veee` (mirrors `.vuma`; preserves the brand in every filename; `.vee` rejected — collapses the "three E's" claim) |
| **Code fence tag** | ` ```veee ` |
| **Compiler binary** | `veeec` (analogous to `rustc`, `clang`) |
| **Language server** | `veee-lsp` (analogous to `rust-analyzer`, `gopls`) |
| **Repo** | `veee` (sibling to `vuma`; separate Rust crate; depends on `vuma` as a Cargo dependency for AST/SCG lowering) |
| **Capitalization convention** | "VEEE" (all caps) when referring to the language/project; "veee" (lowercase) for filenames/commands |
| **Doc site** | `veee.dev` or `veee-lang.org` (both available at audit date) |

**Naming history**: The SWE package used "Vell"/"VELL"/"Vela"
inconsistently (`22-review-against-vuma.md`, `23-vell-redesign-research.md`,
`26-new-plans-three-layers.md`). ADR-0012 renamed VELL → VEEE for four
reasons: phonetic ambiguity (Vell sounds like Vellum/Vela/veil/vial),
branding collision risk (Vellum publishing software, Vela satellites),
explicit VE³ reference (three countable E's), and alignment with the
three-layer architecture (the V ties VEEE to VUMA explicitly). The SWE
package files are preserved as historical artifacts; new docs use VEEE.

---

## 3. Compilation target (from ADR-0014)

**VEEE compiles to VUMA AST (`vuma_parser::ast::AstProgram`), not to VUMA IR.**

After lowering, VUMA's full pipeline runs end-to-end:

```
.veee source
    │
    ▼ veeec (Rust, ~5 kLOC)
VUMA AST (vuma_parser::ast::AstProgram)
    │
    ▼ VUMA pipeline (parse → SCG → IVE → IR → opt → regalloc → codegen)
    │   ├── SCG name resolution + type checking + monomorphization + effect inference
    │   ├── IVE: Z3 contract discharge, session-type linearity, information-flow lattice,
    │   │         PMT invariants, capability verifier
    │   ├── IR + e-graph optimization (3235-line src/codegen/src/egraph.rs)
    │   ├── regalloc (resolve_register_reuse_conflicts for syscalls)
    │   └── 19-backend codegen (x86_64, aarch64, riscv64, wasm32, loongarch64, ...)
    ▼
Native binary (ELF / Mach-O / PE) or Wasm module
```

### 3.1 What VEEE programs inherit from VUMA

By lowering to AST rather than IR, VEEE programs automatically inherit
the full VUMA verification stack:

| VUMA feature | VEEE inherits | How |
|---|---|---|
| **PMT memory-safety verification** | Yes | The arena memory-safety argument (`used + size ≤ capacity`) applies to VEEE-lowered code because it goes through the same SCG → IVE → IR path as hand-written VUMA. |
| **Z3 contract discharge** | Yes | VEEE's `requires`/`ensures` clauses (emitted from monotonicity types) are discharged by Z3 at IVE compile time. |
| **`__oob_trap` runtime bounds check** | Yes | VEEE-lowered `Load`/`Store` instructions get the same runtime `__oob_trap` injection as hand-written VUMA. |
| **Session-type linearity checking** | Yes | VEEE programs that use linear channels (e.g. for IME composition) get the same session-type checking as hand-written VUMA. |
| **Information-flow lattice** | Yes | VEEE programs that use capability-gated resources (e.g. GPU buffers, EditContext) get the same information-flow tracking. |
| **Lean formal spec coverage** | Yes | The Lean `pmt_soundness` theorem (in `proof/PMT/`) models the SCG → IR path. VEEE-lowered code goes through that path, so the theorem applies. |
| **E-graph optimization** | Yes | VEEE-lowered AST nodes enter VUMA's `src/codegen/src/egraph.rs` (3235 lines). VEEE-specific rewrite rules can be added alongside VUMA's existing rules (notably `state_store_load_forward`). |
| **19-backend codegen** | Yes | VEEE programs compile to all 19 backends (x86_64, aarch64, riscv64, wasm32, loongarch64, arm32, mips64, powerpc64, powerpc64le, riscv32, x86_32, sparc64, s390x, mips64be, armeb, aarch64be, m68k, alpha, hppa). |
| **Capability model (HMAC-SHA256)** | Yes | VEEE programs that mint or verify capability tokens go through the same ADR-0007 / ADR-0024 wiring as hand-written VUMA. |

### 3.2 What VEEE's type system must be encodable as

Because VEEE lowers to VUMA AST (not IR), VEEE's type-system features must
be expressible in VUMA's existing type system or encoded as VUMA
contracts. This is a **feature, not a limitation** — it forces VEEE's
type-system design to stay close to VUMA's, which keeps the VEEE compiler
small (~5,000 lines) and the verification story airtight.

| VEEE feature | VUMA encoding |
|---|---|
| `signal T` | `State<T>` (VUMA typed-state API; IVE-verified). The `signal` keyword is VEEE syntax; VUMA never sees it. |
| `derive { ... }` | A VUMA `transform` that reads the dependency `State<T>`s and writes the derived `State<T>`, plus a `mark_dirty` call to WOMB's reactive graph. |
| `monotone set<T>` | A VUMA `layout MonotoneSetT = { version: u64, count: u64, items: [T; 256] }` plus a VUMA `transform insert_item(s: State<MonotoneSetT>, item: T) requires s.count < 256 ensures s.count == old(s.count) + 1 ensures s.version == old(s.version) + 1 { ... }`. The monotonicity *property* (count only grows) is enforced by VEEE's type checker rejecting `remove` calls; the count-advance *contract* is discharged by Z3. |
| `antitone set<T>` | Symmetric: `transform remove_item ... ensures s.count == old(s.count) - 1 ...`. |
| `render { ... }` block | A VUMA `transform` that builds a `SceneNode` tree in the PMT arena (no GPU calls; pure scene construction). |
| `schedule { ... }` block | A VUMA record literal (struct of enum tags) that WOMB's renderer reads at frame time. Not a new IR concept. |
| `gpu kernel` (deferred to v0.2+) | A VEEE source declaration that triggers a build-time `glslangValidator` invocation. The resulting SPIR-V is embedded as a `Lit::Bytes` via `#[embed("file.spv")]` (requires V-26). |
| `match` with guards | Desugars to nested VUMA `match` + `if` (VUMA's `match` doesn't have guards). |
| Closures (`\x -> ...`) | Desugars to VUMA `transform` values (transforms are first-class; monomorphized by `src/codegen/src/monomorphize.rs`). |
| Generics | Desugars to VUMA generics (also monomorphized). |
| Holes (`_`) | Type-check to a typed-hole placeholder that VEEE emits as a runtime trap (exit code 144 — "VEEE hole evaluated at runtime"). The compile-time editor flags the hole; the runtime trap is the defense-in-depth fallback. |

### 3.3 What VEEE can't do (by design)

- **Bypass VUMA's type system.** If VEEE wants a feature VUMA's type system
  can't express, VEEE must either (a) encode it as a contract, (b) desugar
  it to existing VUMA constructs, or (c) propose a VUMA language extension
  (which goes through the VUMA ADR process).
- **Emit raw IR.** VEEE's compiler output is AST, not IR. This is a
  feature, not a limitation — it ensures VEEE programs are always
  verifiable.
- **Skip verification.** VEEE programs always go through IVE. There's no
  `--no-verify` escape hatch for VEEE (VUMA has `--no-verify` for testing,
  but VEEE programs shouldn't use it).

---

## 4. Language design

VEEE's design is **incremental-computation-first**, not HTML-clone. The
SWE package's original VELL sketch (file `23` §1) replicated JSX + CSS
and inherited the web's problems (virtual DOM diffing, cascade/specificity,
imperative event handlers, text-heavy syntax). VEEE repudiates that path.

### 4.1 Incremental computation (from ADR-0016, Salsa/Adapton)

**What VEEE borrows** (from Salsa, used by rust-analyzer):
- Queries memoized by inputs.
- Input → query dependency tracking: when an input changes, only queries
  that read it are invalidated.
- On-demand (lazy): queries only run when their result is read.
- Durable incrementality: results persist across runs (rust-analyzer
  pattern).

**Application to UI**: The UI is a function of state. When state changes,
only the affected subtrees recompute — not the whole tree (virtual DOM
diffing, O(tree)) and not the whole frame (retained-mode full redraw,
O(frame)).

#### VEEE syntax

```veee
-- A root signal: a reactive value that can be read and written.
signal count : i32 = 0

-- A derived signal: re-evaluates only when count changes.
signal label : String = derive { format!("Count: {}", count) }
```

#### Lowering to VUMA

Each VEEE `signal T` lowers to:

1. A VUMA `layout` for the signal's value type with a version counter:
   ```vuma
   layout SignalI32 = { version: u64, value: i32 }
   ```
2. A VUMA `transform` that reads the signal:
   ```vuma
   transform read_signal_i32(s: State<SignalI32>) -> i32 {
       return s.value;
   }
   ```
3. A VUMA `transform` that writes the signal and marks dependents dirty:
   ```vuma
   transform write_signal_i32(s: State<SignalI32>, v: i32) {
       s.value = v;
       s.version = s.version + 1;
       -- mark_dirty is a WOMB call (womb/ui/reactive/graph.vuma)
       mark_dirty(dependents_of(s));
   }
   ```

Each VEEE `derive { ... }` lowers to:
1. A VUMA `transform derive_<name>(deps...) -> T { ... }` that recomputes
   the value from its dependencies.
2. A version-compare guard: at read time, if `dep.version ==
   cached_dep_version`, return the cached value; else recompute.
3. A registration in the compile-time-generated dependency graph (a static
   array of `(signal_id, dependent_id)` pairs stored in the PMT arena).

#### Where each piece lives

| Component | Layer | File |
|---|---|---|
| `signal` / `derive` syntax | VEEE | `veeec` parser |
| Dependency graph construction | VEEE | `veeec` reactivity analyzer pass |
| `State<T>` typed-state API | VUMA | `src/codegen/src/marshal.rs`, `scg_to_ir.rs` |
| `mark_dirty` graph walker | WOMB | `womb/ui/reactive/graph.vuma` (greenfield) |
| Memoization cache | WOMB | `womb/ui/reactive/memo.vuma` (greenfield) |
| Cycle detection | VEEE | `veeec` type checker (compile-time, rejects cyclic `derive` chains) |

**Key design choice** (ADR-0016): the incremental-computation engine
lives in VEEE, not in VUMA. VUMA gains no new `Signal` IR instruction, no
new `Effect::Incremental` variant, no new Lean proofs. VUMA just sees
`State<SignalI32>` reads and writes, which it verifies like any other
state access. This keeps VUMA minimal (~5,000 lines of VUMA code avoided)
and lets VEEE iterate on the algorithm (Salsa vs. Adapton) without a VUMA
release.

**Reactivity verification**: signal reads/writes are `State<T>` accesses,
so they get PMT memory-safety verification for free. The reactivity
property itself (dependents re-evaluate after dependency changes) is a
VEEE-layer invariant, not a VUMA-layer invariant. It is enforced by
VEEE's type checker and the compile-time-generated dependency graph, not
by Z3. This is the same guarantee React/SwiftUI/Compose provide (they
don't formally verify reactivity either) — but VEEE provides it on top of
PMT memory safety, which they don't provide at all.

### 4.2 Monotonicity types (from ADR-0017, Datafun)

**What VEEE borrows** (from Datafun, Arntzenius & Krishnaswami, ICFP 2016;
seminaïve extension 2020):
- Sets are first-class; collection operations (map, filter, join) return
  sets.
- **Monotonicity types**: the type system tracks which collections only
  grow (`monotone`) and which only shrink (`antitone`). This enables
  **seminaïve evaluation** — when a monotone collection changes, you only
  re-evaluate the parts that depend on the *new* elements, not the whole
  collection.
- Datalog-style fixpoint recursion with termination guarantees.

**Application to UI**: A list of items is a set. When you add an item, you
shouldn't re-render the whole list — you should compute the delta (one new
item) and render only that. React does this ad-hoc (the `key` prop); VEEE
makes it principled via the type system.

#### VEEE syntax

```veee
monotone set visible_items : Set<ItemId>      -- can only grow
antitone set hidden_items  : Set<ItemId>      -- can only shrink

-- A derived collection. Monotone in visible_items, antitone in hidden_items.
derive filtered : Set<Item> =
  visible_items.map(|id| lookup(id))
               .filter(|i| !hidden_items.contains(i.id))
```

#### Type-checker rules

The VEEE type checker enforces:

- `visible_items.insert(x)` is **allowed** (monotone grow).
- `visible_items.remove(x)` is **rejected** at compile time (monotone shrink).
- `hidden_items.remove(x)` is **allowed** (antitone shrink).
- `hidden_items.insert(x)` is **rejected** at compile time (antitone grow).
- `filtered`'s derivation is verified monotone in `visible_items` and
  antitone in `hidden_items`. So adding to `visible_items` or removing
  from `hidden_items` only *adds* to `filtered` — never removes.

#### Lowering to VUMA

Each monotone collection lowers to:

1. A VUMA `layout` with a version counter:
   ```vuma
   layout MonotoneSetItemId = {
       version: u64,
       count: u64,
       items: [ItemId; 256]
   }
   ```
2. A VUMA `transform` for insertion (with `ensures` contract):
   ```vuma
   transform insert_item(s: State<MonotoneSetItemId>, item: ItemId)
       requires s.count < 256
       ensures s.count == old(s.count) + 1
       ensures s.version == old(s.version) + 1
   {
       s.items[s.count] = item;
       s.count = s.count + 1;
       s.version = s.version + 1;
   }
   ```
3. Z3 discharges the `ensures` clauses, proving the count and version
   advance correctly. (Trivial for Z3.)

The runtime uses the version counter to detect changes. When `filtered`
is re-evaluated, it compares the versions of `visible_items` and
`hidden_items` to its cached versions. If unchanged, return the cache. If
changed, re-evaluate only the delta (the new elements in
`visible_items` or the removed elements from `hidden_items`).

#### Where each piece lives

| Component | Layer | File |
|---|---|---|
| `monotone set` / `antitone set` syntax | VEEE | `veeec` parser |
| Monotonicity inference + type checking | VEEE | `veeec` type checker (~3,000 lines of Rust) |
| Seminaïve delta evaluation | VEEE | `veeec` lowering pass (emits two transforms: initial fill + delta) |
| `requires`/`ensures` contract discharge | VUMA | `src/ive/src/verification.rs` (`discharge_contracts_and_prove_blocks`) |
| Monotone set data structure | WOMB | `womb/collections/monotone_set.vuma` (greenfield) |
| Delta-tracking helpers | WOMB | `womb/collections/delta.vuma` (greenfield) |

**Why VEEE-layer, not VUMA-layer** (ADR-0017): Monotonicity is a
type-system property, not a memory-layout property. VUMA's `IRType` is
about memory layout (`I32`, `F64`, etc.). Adding `Monotone<T>` to `IRType`
would conflate the type-system level (compile-time checking) with the IR
level (runtime representation). VUMA's IVE verifies memory safety
(`used + size ≤ capacity`), not collection semantics (`count only grows`).
These are different properties at different abstraction levels. Keeping
monotonicity in VEEE preserves the 5-crate policy, keeps VUMA's IR
domain-neutral, and lets VEEE iterate on the type system without a VUMA
release.

### 4.3 Algorithm/schedule separation (from Halide)

**What VEEE borrows** (from Halide, Adobe Research + MIT):
- Separate the **algorithm** (what to compute: paths, colors, transforms)
  from the **schedule** (how to compute it: GPU passes, tessellation
  strategy, buffer layout).
- The compiler explores the schedule space (autotuning) and generates
  optimized code per target.

**Application to UI rendering**: A UI scene is an algorithm (these paths,
these colors, these transforms). The rendering schedule (which GPU passes,
what tessellation strategy, what buffer layout) is a separate concern.
Current UI frameworks bake the schedule into the algorithm (you write
`draw_quad()` calls). Halide-style separation lets the compiler choose
the optimal schedule per platform.

#### VEEE syntax

```veee
-- ALGORITHM: what to render (declarative scene tree)
render app =
  window "Counter" (400, 300) [
    counter_view,
    item_list
  ]

-- SCHEDULE: how to render (a record value WOMB's renderer interprets)
schedule {
  tessellation: gpu_compute,    -- vs. cpu_fallback
  batching: per_material,        -- vs. per_node
  scroll: composited_layer,      -- vs. translate_paths
  text: path_per_glyph           -- vs. cached_atlas
}
```

#### Lowering to VUMA

- The `render` block lowers to a VUMA `transform` that builds a `SceneNode`
  tree in the PMT arena. This transform emits **no GPU calls** — it just
  constructs the scene description (paths, transforms, colors, clip
  stacks). The `SceneNode` layout is defined by WOMB
  (`womb/ui/render/scene.vuma`).
- The `schedule` block lowers to a VUMA **record literal** — a struct of
  enum tags (`Tessellation::GpuCompute`, `Batching::PerMaterial`, etc.).
  WOMB's renderer reads this record at frame time and configures itself
  accordingly.
- VUMA's IR gains no new "schedule" concept. The schedule is a value (a
  struct of enum tags) that the renderer interprets. This is the cleanest
  mapping: VEEE's schedule DSL is sugar for a record constructor.

#### What's deferred to v0.2+

- **GPU schedule autotuning** (Halide-style schedule-space exploration at
  build time). The v0.1 schedule is static per build target. Autotuning
  is a v0.2+ feature; the VEEE team can experiment with it without a VUMA
  release because the schedule is just a value.

### 4.4 E-graph optimization

**What VEEE borrows**: VEEE → VUMA AST → VUMA's existing e-graph
(`src/codegen/src/egraph.rs`, 3235 lines). VEEE-specific rewrite rules
are added alongside VUMA's existing PMT-state rewrites.

**Verification** (K-1, against actual source):
- `src/codegen/src/egraph.rs` exists. 3235 lines.
  - `pub enum ENode` at line 81.
  - `pub struct EGraph` at line 141.
  - `pub fn new()` at 165, `pub fn add()` at 176, `pub fn find()` at
    193, `pub fn merge()` at 206, `pub fn rebuild()` at 250.
  - File docstring (lines 1–60) documents: congruence-closure rebuild,
    bottom-up DP extraction, commutativity/associativity/distributivity
    rules, **PMT state-operation ENodes** including `StateInit`,
    `StateRead`, `StateWrite`, `StateTransform`, plus rewrite rules
    `state_dead_init_elim`, **`state_store_load_forward`**.
- `src/codegen/src/bv_verify.rs` exists (bitvector verification —
  proof-carrying e-graph rewrite rules).
- `src/codegen/src/proof_artifacts.rs` exists (proof artifact emission
  for verified rewrites).
- `tests/egraph_extraction_tests.rs` exists (test coverage).

**The key insight**: the `state_store_load_forward` rewrite is *exactly*
what VEEE needs for cheap signal-change reactivity. When a `signal` is
written and then read in the same frame (which is common — a UI event
handler writes the signal, then the renderer reads it), the e-graph
simplifies the store-load pair to a direct value use. VEEE doesn't have
to emit this optimization; it gets it for free from VUMA's existing
egraph pass.

**VEEE-specific rewrite rules** (added alongside VUMA's, in a new
`veeec` module that contributes rules to the e-graph):

| Rule | What it does | Why VEEE needs it |
|---|---|---|
| `derive_constant_fold` | If a `derive` block's dependencies are all compile-time constants, fold the derivation to a constant. | Avoids runtime recomputation of derived signals whose inputs never change. |
| `signal_dead_write_elim` | If a signal is written but never read (no `derive` depends on it, no `render` block reads it), eliminate the write. | Removes the `mark_dirty` call and the version bump. |
| `monotone_set_idempotent_insert` | If `insert(x)` is called twice on a monotone set without `x` being removed in between, the second insert is a no-op (the set is monotone, so `x` is already present). | Avoids duplicate work in seminaïve delta evaluation. |
| `derive_fuse` | Two `derive` blocks with the same dependency and the same body fuse into one. | Avoids double re-evaluation of identical derived signals. |

All four rules are **pure e-graph rewrites** — they don't add IR
instructions; they just simplify existing ones. They're added in a
VEEE-specific rewrite-rule file that the VEEE compiler registers with
VUMA's e-graph at compile time. (Roughly ~500 lines of Rust.)

### 4.5 Structured-editing-friendly syntax (with 5 example programs)

The syntax is designed for **projectional editors** (Hazel-style live
editing), not just text parsers. The design constraints:

1. **Record-based styles** — every style is a flat record (`[bg #06C,
   radius 4]`), parseable as an AST node with named children. No CSS
   cascade, no specificity, no parser ambiguity.
2. **Pure state transitions** — every event handler is a single assignment
   expression (`(on_click => count := count + 1)`). No imperative block,
   no statement list.
3. **No nested precedence** — function application is the only binding
   form; no operator-precedence parser needed.
4. **Holes are syntactically valid** — `_` in any expression position is a
   typed hole (Hazel pattern). The editor can always render a valid AST.
5. **No statement vs. expression distinction** — everything evaluates to a
   value. (The `:=` operator returns `unit`.)

#### Example 1: Counter (the canonical VEEE hello-world)

```veee
-- counter.veee
-- A root signal, a derived signal, and a pure state-transition handler.

signal count : i32 = 0
signal label : String = derive { format!("Count: {}", count) }

ui counter_view =
  column [spacing 8] [
    text label [font_size 16, color #333333],
    button "Increment"
      [bg #0066CC, color white, radius 4, padding (8, 16)]
      (on_click => count := count + 1),
    button "Reset"
      [bg #CC0000, color white, radius 4, padding (8, 16)]
      (on_click => count := 0)
      (disabled => count == 0)
  ]

render app =
  window "Counter" (400, 300) [
    counter_view
  ]

schedule {
  tessellation: gpu_compute,
  batching: per_material,
  scroll: translate_paths,
  text: path_per_glyph
}
```

**What this lowers to** (sketch — the actual lowering is in §5):

- `signal count : i32 = 0` → `layout SignalI32 = { version: u64, value: i32 }`
  + `transform read_signal_i32/write_signal_i32`.
- `signal label : String = derive { ... }` → `transform derive_label(count: State<SignalI32>) -> String`
  + registration in the dependency graph (`(count_id, label_id)`).
- `ui counter_view = column [...]` → `transform counter_view(count: State<SignalI32>) -> SceneNode`
  that builds a column scene node with two button children and one text
  child. The `on_click` handler lowers to a closure passed to WOMB's
  button primitive; the closure calls `write_signal_i32(count, count + 1)`.
- `render app = window [...]` → `transform render_app(...) -> SceneNode`
  that builds a window scene node with `counter_view` as its child.
- `schedule { ... }` → a VUMA record literal:
  ```vuma
  let sched : RenderConfig = RenderConfig {
      tessellation: Tessellation::GpuCompute,
      batching: Batching::PerMaterial,
      scroll: Scroll::TranslatePaths,
      text: Text::PathPerGlyph
  };
  ```

The e-graph then runs. Notably:
- `derive_constant_fold` doesn't fire (count is a root signal, not a
  constant).
- `state_store_load_forward` fires on the `on_click => count := count + 1`
  handler if the next operation reads `count` (the renderer does, via
  `label`). The store-load pair simplifies to a direct value use.

#### Example 2: Todo list (monotonicity types in action)

```veee
-- todo.veee
-- Demonstrates monotone/antitone sets and seminaïve delta evaluation.

layout TodoItem = { id: u64, text: String, done: bool }

monotone set todos    : Set<TodoItem>      -- can only grow
antitone set deleted  : Set<u64>           -- can only shrink (tombstones)

derive active : Set<TodoItem> =
  todos.filter(|t| !deleted.contains(t.id))

signal new_todo_text : String = ""

ui todo_list =
  column [spacing 4] [
    text_input new_todo_text
      [placeholder "What needs doing?"]
      (on_submit => {
        todos := todos ∪ { TodoItem { id: next_id(), text: new_todo_text, done: false } };
        new_todo_text := ""
      }),
    column [] (map active (\item ->
      row [spacing 8] [
        checkbox item.done
          (on_change => todos := todos ∪ { TodoItem { ..item, done: !item.done } }),
        text item.text [],
        button "Delete"
          []
          (on_click => deleted := deleted ∪ { item.id })
      ]
    ))
  ]

render app =
  window "Todos" (600, 400) [
    todo_list
  ]

schedule { tessellation: gpu_compute, batching: per_node, text: path_per_glyph }
```

**Why monotonicity matters here**: When the user clicks "Delete" on item
#5, the `deleted` set grows (it's monotone, so `insert` is allowed). The
`active` derived set is antitone in `deleted` (adding to `deleted` removes
from `active`). Seminaïve delta evaluation:
- `delta(active) = - { item #5 }` (only the removed item).
- `delta(todo_list UI) = remove the row for item #5` (only that row).

The other rows don't re-evaluate. Compare React: the whole `todo_list`
component re-runs, then the virtual DOM diff finds the missing row. VEEE
skips both steps.

**Type-checker enforcement**:
- `todos.insert(x)` — allowed (monotone grow).
- `todos.remove(x)` — **rejected** at compile time. (You can't remove
  from `todos`; you can only mark the item as deleted via the `deleted`
  antitone set. This is the tombstone pattern, formalized by the type
  system.)
- `deleted.insert(x)` — allowed (antitone grow? No — antitone means
  "shrinks over time", but `insert` is a grow. So actually, `deleted`
  should be **monotone** too. Let me reread ADR-0017...

Actually, looking at ADR-0017 more carefully: "antitone set" means "can
only shrink". So `deleted` would be... a monotone set (it only grows —
tombstones accumulate). The semantics are: `todos` grows monotonically
(you only ever add todos); `deleted` also grows monotonically (you only
ever add tombstones). The *derived* set `active = todos.filter(|t| !
deleted.contains(t.id))` is monotone in `todos` (adding to `todos` adds
to `active`) and antitone in `deleted` (adding to `deleted` removes from
`active`). The monotonicity *of the derived* is what the type checker
verifies.

Let me fix the example:

```veee
-- todo.veee (corrected)
-- `todos` is monotone (only grows — you can only add todos).
-- `deleted` is monotone (only grows — tombstones accumulate).
-- `active` is derived: monotone in todos, antitone in deleted.

layout TodoItem = { id: u64, text: String, done: bool }

monotone set todos    : Set<TodoItem>
monotone set deleted  : Set<u64>

derive active : Set<TodoItem> =
  todos.filter(|t| !deleted.contains(t.id))

-- ... rest as above
```

Now the type checker:
- `todos.insert(x)` — allowed (monotone grow).
- `deleted.insert(x)` — allowed (monotone grow).
- `todos.remove(x)` — rejected (monotone shrink).
- `deleted.remove(x)` — rejected (monotone shrink).
- `active`'s derivation is verified monotone in `todos` (filter preserves
  monotonicity: adding to `todos` can only add to `active`) and antitone
  in `deleted` (adding to `deleted` can only remove from `active`).

#### Example 3: Text label (minimal WOMB interop)

```veee
-- label.veee
-- The simplest non-trivial VEEE program: a text label.

signal text : String = "Hello, VEEE!"

ui label_view =
  text text [font_size 24, color #000000, weight bold]

render app =
  window "Label" (300, 100) [
    label_view
  ]

schedule { text: path_per_glyph, tessellation: cpu_fallback }
```

**What this exercises**:
- `signal text : String` lowers to `State<SignalString>` (where
  `SignalString` is a length-prefixed byte buffer in the PMT arena).
- `ui label_view = text text [...]` lowers to a call to WOMB's
  `womb/ui/text/shaper_v1.vuma` (which uses cmap + hmtx, f32 advances,
  per `26` Phase W-2) followed by a scene-tree node construction.
- `schedule { text: path_per_glyph, ... }` tells WOMB's renderer to
  shape each glyph into a path (vector renderer, no glyph atlas — per
  `26` Phase W-5) and tessellate on the CPU (the `cpu_fallback` schedule
  is for headless / CI environments without GPU access).

This is the smallest end-to-end VEEE program: signal → derive (none
here) → UI → render → schedule. It exercises the full VEEE → VUMA → WOMB
pipeline.

#### Example 4: Simple animation (time as a signal)

```veee
-- animation.veee
-- A box that pulses (scales up and down) over 1 second.

signal t : f32 = 0.0                    -- seconds, advanced by host clock
signal scale : f32 = derive {
  let phase = (t * 2.0 * 3.14159).sin();  -- 1 Hz sine wave
  1.0 + 0.2 * phase                       -- scales between 0.8 and 1.2
}

ui pulse_view =
  rect [
    bg #0066CC,
    width (100.0 * scale),
    height (100.0 * scale),
    radius 8
  ] []

-- The host (browser RAF or native vsync) advances `t` each frame.
-- WOMB's frame scheduler (womb/ui/render/frame.vuma) calls:
--   write_signal_f32(t, host_clock_seconds());

render app =
  window "Pulse" (200, 200) [
    pulse_view
  ]

schedule { tessellation: gpu_compute, animation: host_driven }
```

**What this exercises**:
- `signal t : f32` requires V-34 (DONE per the catalog: `bridge_type_to_ir_type`
  maps `"f32"` → `IRType::F32`). f32 signals work.
- `derive { ... }` with a `let` binding and arithmetic — VEEE's derive
  blocks are full expression-oriented VEEE syntax, not a restricted
  subset.
- The `host_driven` animation schedule tells WOMB's frame scheduler to
  advance `t` from the host clock (browser `requestAnimationFrame` timestamp
  or native vsync). VEEE doesn't implement the clock; WOMB does
  (`womb/ui/render/frame.vuma`).
- The e-graph's `state_store_load_forward` rule fires on every frame:
  `t` is written by the host, then read by `scale`'s derivation, then
  read by `pulse_view`'s render. The store-load pairs simplify, so the
  frame's work is: one f32 write, one sin computation, one rect scene
  node update. No redundant loads.

**Note on f32 PMT verification**: V-14 (Lean proof for f32 arithmetic)
is deferred to v2 (per `19-open-questions.md` Q-01 and the catalog).
v1 uses the runtime `__float_overflow_trap` (exit 142) for NaN/inf in
f32 arithmetic. VEEE programs that use f32 signals get the runtime trap,
not the Lean proof. This is documented in caveats.md §2.x.

#### Example 5: GPU shader dispatch (path tessellation)

```veee
-- gpu_render.veee
-- A VEEE program that dispatches a hand-written GLSL path-tessellation
-- shader via WOMB's gpu_dispatch host import.

-- The shader is pre-compiled to SPIR-V at build time by glslangValidator.
-- V-26 (const byte arrays) lets VEEE embed it as a Lit::Bytes.
const TESSELLATE_PATH_SPIRV : [u8; 4096] = #[embed("shaders/path_tessellate.spv")]

signal path_data : Buffer<PathSegment> = Buffer::new(1024)
signal vertices  : Buffer<Vertex>     = Buffer::new(2048)

ui vector_canvas =
  gpu_render_pass TESSELLATE_PATH_SPIRV
    [path_data, vertices]
    [workgroup (256, 1, 1)]

render app =
  window "Vector Canvas" (800, 600) [
    vector_canvas
  ]

schedule {
  tessellation: gpu_compute,    -- dispatches the SPIR-V shader
  batching: per_material,
  text: path_per_glyph
}
```

**Build-time flow** (per ADR-0022):

```
shaders/path_tessellate.comp.glsl  (~500 LOC hand-written GLSL)
    │
    ▼ glslangValidator (BSD-3, build-time only)
shaders/path_tessellate.spv
    │
    ▼ veeec build script (Python or shell, ~50 LOC)
shaders/path_tessellate.spv (bytes)
    │
    ▼ #[embed("shaders/path_tessellate.spv")] attribute
VUMA AST: const TESSELLATE_PATH_SPIRV : [u8; 4096] = [/* 4096 bytes */]
    │
    ▼ VUMA pipeline (parse → SCG → IVE → IR → codegen → .rodata)
VUMA binary with embedded SPIR-V blob in .rodata
    │
    ▼ Runtime: WOMB renderer calls gpu_dispatch(TESSELLATE_PATH_SPIRV, ...)
Native GPU execution (Vulkan / Metal / WebGPU)
```

**What VEEE provides**:
- The `#[embed("file.spv")]` attribute (VEEE syntax).
- The build script that invokes `glslangValidator` on `.glsl` files and
  produces `.spv` files.
- The `gpu_render_pass` UI primitive (in VEEE's standard library), which
  lowers to a call to WOMB's `gpu_dispatch` host import.

**What WOMB provides**:
- `womb/ui/render/gpu_dispatch.vuma` — the `gpu_dispatch` host import:
  ```vuma
  extern "C" {
      transform vk_create_compute_pipeline_spirv(
          device: Address, spirv: Address, spirv_len: i64
      ) -> i64;
      transform vk_cmd_bind_pipeline(cmd: Address, pipeline: i64) -> i32;
      transform vk_cmd_dispatch(cmd: Address, x: i32, y: i32, z: i32) -> i32;
  }
  ```
- `womb/ui/render/vector.vuma` — the path-tessellation renderer that calls
  `gpu_dispatch` with the embedded SPIR-V blob.

**What VUMA provides** (small patch, deferred):
- **V-26**: Const byte arrays — parser support for `Lit::Bytes(Vec<u8>)`
  and `Expr::ArrayLit`. ~2 weeks. This is the **only** VUMA-side patch
  needed for the GPU path.
- The `.rodata` lowering (already exists for string literals; extend to
  byte arrays).

**What VEEE does NOT do**:
- No VEEE GPU DSL. The shader is hand-written GLSL, not VEEE. (A VEEE
  GPU DSL is deferred to v0.2+; it would lower to GLSL source, not MLIR,
  per ADR-0022's rejection of the MLIR approach.)
- No runtime shader compilation. The SPIR-V is pre-compiled at build
  time.
- No PMT verification of the shader. PMT is a CPU memory model; GPU
  kernels use a different memory model (workgroup-shared, global, image).
  GPU kernel verification is out of scope for v1. Runtime `spirv-val`
  validation (run at build time, after `glslangValidator`) catches
  shader bugs before runtime.

---

## 5. Compiler architecture

### 5.1 VEEE compiler (`veeec`, written in Rust)

The VEEE compiler is a single Rust binary, `veeec`, that lives in the
`veee` repo (sibling to `vuma`). It depends on `vuma` as a Cargo
dependency for AST/SCG lowering — specifically, it uses
`vuma_parser::ast` (the AST data structures) and
`vuma::pipeline::compile_modules` (the integration point that takes AST
and runs the full VUMA pipeline).

#### Compiler pipeline

```
.veee source
    │
    ▼ 1. Lexer (hand-written NFA, no `logos` crate — VUMA philosophy)
VEEE tokens
    │
    ▼ 2. Parser (recursive descent, no `nom` or `pest` — VUMA philosophy)
VEEE AST (veee::ast::VeeeProgram)
    │
    ▼ 3. Type checker
    │    ├── Signal/derive dependency graph construction
    │    ├── Cycle detection (rejects cyclic `derive` chains)
    │    ├── Monotonicity inference + enforcement
    │    └── Effect inference (delegates to VUMA's effect system post-lowering)
Typed VEEE AST
    │
    ▼ 4. Reactivity analyzer
    │    ├── Builds the compile-time dependency graph (static array of
    │    │   (signal_id, dependent_id) pairs)
    │    ├── Emits `mark_dirty` calls into the lowered VUMA code
    │    └── Emits version-compare guards for `derive` blocks
Annotated VEEE AST
    │
    ▼ 5. Lowering pass (VEEE AST → VUMA AST)
    │    ├── signal T → layout SignalT + read/write transforms
    │    ├── derive { ... } → transform + version-compare guard
    │    ├── monotone set T → layout MonotoneSetT + insert/remove transforms
    │    │   (with requires/ensures contracts for Z3 discharge)
    │    ├── render { ... } → transform building a SceneNode tree
    │    ├── schedule { ... } → record literal of enum tags
    │    ├── ui name = ... → transform producing a SceneNode
    │    ├── closures → VUMA transform values
    │    ├── generics → VUMA generics (monomorphized by VUMA)
    │    ├── match with guards → nested VUMA match + if
    │    └── #[embed("file")] → VUMA Lit::Bytes (requires V-26)
VUMA AST (vuma_parser::ast::AstProgram)
    │
    ▼ 6. VEEE-specific e-graph rewrite rules registered
    │    (derive_constant_fold, signal_dead_write_elim,
    │     monotone_set_idempotent_insert, derive_fuse)
VUMA AST + VEEE rewrite rules
    │
    ▼ 7. vuma::pipeline::compile_modules (the integration point)
    │    VUMA's full pipeline: SCG → IVE → IR → e-graph → regalloc → codegen
Native binary or Wasm module
```

#### Module structure (estimated ~5,000 lines of Rust)

```
veee/
  Cargo.toml                  (depends on vuma)
  src/
    main.rs                   (~200 LOC) — CLI entry point
    lexer.rs                  (~400 LOC) — hand-written NFA lexer
    parser.rs                 (~800 LOC) — recursive-descent parser
    ast.rs                    (~500 LOC) — VEEE AST data structures
    type_check.rs             (~1,500 LOC) — type checker + monotonicity inference
                                           (the bulk of the compiler)
    reactivity.rs             (~600 LOC) — dependency graph + mark_dirty emission
    lower.rs                  (~800 LOC) — VEEE AST → VUMA AST lowering
    egraph_rules.rs           (~200 LOC) — VEEE-specific e-graph rewrite rules
    embed.rs                  (~100 LOC) — #[embed("file")] attribute handler
  std/                        (VEEE standard library, see §6)
    layout.veee
    text.veee
    render.veee
    reactive.veee
    ...
  shaders/                    (hand-written GLSL, see §7)
    path_tessellate.comp.glsl
    path_rasterize.frag.glsl
  build.rs                    (~50 LOC) — invokes glslangValidator on .glsl files
```

**Why ~5,000 lines, not ~15,000** (per ADR-0014): VEEE doesn't duplicate
VUMA's type checker, effect inferencer, verifier, optimizer, or codegen.
It only does:
- Parse `.veee` source.
- Type-check VEEE-specific features (signals, monotonicity, derive).
- Lower to VUMA AST.
- Register VEEE-specific e-graph rules.

Everything else is VUMA's job. If VEEE lowered to IR directly (the
rejected Option B from ADR-0014), it would have to duplicate VUMA's type
checker (~10,000 lines), effect inferencer, and verifier — and would lose
PMT verification in the process.

#### No separate IR, no separate optimizer

VEEE has **no intermediate representation** between its AST and VUMA's
AST. The lowering pass goes directly from `veee::ast::VeeeProgram` to
`vuma_parser::ast::AstProgram`. This is by design — VEEE's value
proposition is that it sits *on top of* VUMA, not *alongside* it. Any
intermediate IR would be a duplication of VUMA's AST.

VEEE has **no separate optimizer**. VUMA's e-graph
(`src/codegen/src/egraph.rs`) handles optimization. VEEE contributes
rewrite rules but doesn't run its own optimization pass. This is by
design — the e-graph is shared infrastructure, and running two
optimizers would mean two sources of bugs.

### 5.2 Backend strategy (from ADR-0023, ADR-0022)

VEEE has **three compilation tracks**, all of which use VUMA's existing
codebase. **No new Rust crates. No Cranelift. No MLIR. No LLVM.** The
5-crate external-dependency policy (ADR-0010) is preserved.

#### Track 1: Dev builds — VUMA codegen with `--dev` flags

VEEE's `veeec` accepts a `--dev` flag that sets `CompileConfig` fields to
skip expensive optimizations while keeping verification ON:

| `CompileConfig` field | `--dev` value | Default (prod) | Effect |
|---|---|---|---|
| `opt_level` | `OptLevel::None` | `OptLevel::Aggressive` | Skips e-graph rewriting, loop unrolling, vectorization, PGO, LTO |
| `verification_level` | `VerificationLevel::Pmt` (unchanged) | `VerificationLevel::Pmt` | Verification stays ON — dev builds are still verified |
| `lto` | `false` | `true` | No link-time optimization |
| `codegen_units` | `16` | `1` | Parallel codegen (faster, less optimized) |
| `target` | Host ISA only | Cross-compile to all 19 | No cross-arch codegen in dev mode |

**Why this is sufficient** (ADR-0023): VUMA's codegen is already fast for
unoptimized builds. The 19-backend codegen is slow only because of the
optimization passes. With `opt_level=None`, a typical VUMA program
compiles in ~1-2 seconds — competitive with the rejected Cranelift
approach.

**Measured baseline**: `tests/gold_standard/float_advanced/fp_bench.vuma`
compiles in ~1.3s with `opt_level=None` (verified by ADR-0023). This is
the benchmark the VEEE team should re-run on a typical VEEE program
(counter.veee, ~50 LOC) to confirm the dev-build experience is
acceptable.

**Verification stays ON in dev** — this is non-negotiable. VEEE's value
proposition is verified UI. Turning off verification in dev mode would
defeat the purpose. The `--dev` flag skips *optimization*, not
*verification*.

**No e-graph in dev mode** — with `opt_level=None`, the e-graph rewriter
is skipped. VEEE's incremental-computation optimization relies on the
e-graph (specifically `state_store_load_forward`). In dev mode, VEEE
programs may be slower at runtime (but still correct). Acceptable for
dev.

#### Track 2: Production builds — VUMA's 19-backend codegen

Unchanged from VUMA's existing pipeline. VEEE programs get the full
optimization suite:
- E-graph rewriting (including VEEE-specific rules: `derive_constant_fold`,
  `signal_dead_write_elim`, `monotone_set_idempotent_insert`, `derive_fuse`).
- Loop unrolling, vectorization (SSE2/AVX2/NEON, per ADR-0025).
- PGO (profile-guided optimization).
- LTO (link-time optimization).
- Cross-compilation to all 19 backends.
- Full PMT verification + Lean formal spec coverage.

#### Track 3: GPU shaders — hand-written GLSL → glslangValidator → SPIR-V → embed

Per ADR-0022 (Accepted, supersedes the rejected ADR-0018 MLIR approach):

```
VEEE GPU kernel (GLSL source, hand-written, NOT a VEEE DSL)
    │
    ▼ Build-time: glslangValidator (BSD-3, build-time only, NOT a runtime dep)
SPIR-V bytecode (.spv file)
    │
    ▼ Build-time: veeec build script (Python or shell, ~50 LOC)
Const byte array literal (V-26: [u8; N] = [/* ... */])
    │
    ▼ VEEE compiler embeds in VUMA AST via #[embed("file.spv")]
VUMA AST (Lit::Bytes or Expr::ArrayLit)
    │
    ▼ VUMA pipeline (parse → SCG → IVE → IR → codegen → .rodata)
VUMA binary with embedded SPIR-V blob in .rodata
    │
    ▼ Runtime: WOMB renderer calls Vulkan/Metal/WebGPU host import
Native GPU execution
```

**What glslangValidator is**: a BSD-3-licensed C++ tool from the
KhronosGroup SPIR-V tools. It compiles GLSL → SPIR-V. It's a build-time
tool, like Z3 (the SMT solver VUMA already uses for IVE). It's installed
on the build host but **not linked into the output binary**. The 5-crate
policy applies to Rust crates, not build tools.

**What is NOT used** (and why):

| Tool | Why not |
|---|---|
| **MLIR** (the rejected ADR-0018 approach) | C++ toolchain, massive, violates VUMA's hand-write philosophy, no e-graphs benefit for shaders. ADR-0022 explicitly rejects this. |
| **Cranelift** (the rejected ADR-0015 dev-build approach) | Rust crate (would be 6th dep, violating ADR-0010); also unnecessary — VUMA's `--dev` flags are sufficient. ADR-0023 explicitly rejects this. |
| **LLVM** | C++, no e-graphs, violates hand-write philosophy. |
| **wgpu / gfx-rs** | Rust crates, violate "pure VUMA kernel, no Rust crates in kernel" (SWE package `16-build-vs-buy.md`). |
| **shaderc / glslang rust bindings** | Rust crate wrappers around glslang; use the C tool directly via build script instead. |
| **Runtime SPIR-V compilation** | Slow (50-200ms per shader), loses compile-time validation. |

**Consistency with VUMA's philosophy**: VUMA hand-writes everything —
lexer NFA (no `logos`), TOML parser (no `toml`), JSON (no `serde_json`),
HMAC-SHA256 (no `sha2`/`hmac` crates), 19 backends (no LLVM), e-graph
(no `egg` crate). VEEE inherits this philosophy. The only new tool is
`glslangValidator`, which is a build-time tool (like Z3), not a Rust
crate.

---

## 6. VEEE standard library

VEEE's standard library is a **thin wrapper over WOMB UI primitives**. It
provides ergonomic VEEE syntax for common UI patterns, but the actual
work is done by WOMB modules (which are VUMA source, PMT-verified).

**Design principle**: No duplication of WOMB functionality. VEEE is
syntactic sugar + a type system + reactivity. If a feature belongs in
the UI engine, it goes in WOMB. If a feature belongs in the language, it
goes in VEEE.

### 6.1 Standard library modules

| VEEE module | Wraps | WOMB module | Status |
|---|---|---|---|
| `veee::layout::flex` | Flexbox layout | `womb/ui/layout/flex.vuma` | Greenfield (W-4) |
| `veee::layout::stack` | Stacking contexts | `womb/ui/layout/stacking.vuma` | Greenfield (W-4) |
| `veee::layout::scroll` | Scroll containers | `womb/ui/layout/scroll.vuma` | Greenfield (W-4) |
| `veee::text::label` | Text label | `womb/ui/text/shaper_v1.vuma` | Greenfield (W-2) |
| `veee::text::input` | Text input field | `womb/ui/ime/textfield.vuma` | Greenfield (W-6) |
| `veee::render::scene` | Scene tree construction | `womb/ui/render/scene.vuma` | Greenfield (W-5) |
| `veee::render::vector` | Vector renderer | `womb/ui/render/vector.vuma` | Greenfield (W-5) |
| `veee::render::gpu_dispatch` | GPU shader dispatch | `womb/ui/render/gpu_dispatch.vuma` | Greenfield (W-5) |
| `veee::reactive::signal` | Signal primitive | (lowers to VUMA `State<T>` + WOMB `mark_dirty`) | V-34 DONE, WOMB reactive graph greenfield |
| `veee::reactive::derive` | Derived signal | (lowers to VUMA transform + version-compare guard) | VEEE-internal |
| `veee::collections::monotone_set` | Monotone set | `womb/collections/monotone_set.vuma` | Greenfield |
| `veee::collections::antitone_set` | Antitone set | `womb/collections/monotone_set.vuma` (symmetric) | Greenfield |
| `veee::animation::tween` | Animation tweening | `womb/ui/animation.vuma` | Greenfield (W-4) |
| `veee::theme::manager` | Theme manager | `womb/ui/theme.vuma` | Greenfield (W-4) |
| `veee::a11y::semantics` | Accessibility tree | `womb/ui/a11y/semantics.vuma` | Greenfield (W-7) |
| `veee::event::dispatcher` | Event dispatcher | `womb/ui/event/dispatch.vuma` | Greenfield (W-0) |

### 6.2 Example: `veee::layout::flex`

The VEEE `column` / `row` primitives are syntactic sugar for WOMB's
Flexbox layout:

```veee
-- VEEE source
ui counter_view =
  column [spacing 8] [
    text label [...],
    button "Increment" [...]
  ]
```

Lowers to (sketch):

```vuma
-- Generated VUMA
transform counter_view(count: State<SignalI32>) -> SceneNode {
    let label_node = text_node(read_signal_i32(count), TextStyle { ... });
    let button_node = button_node("Increment", ButtonStyle { ... }, /* on_click */ \() -> {
        write_signal_i32(count, read_signal_i32(count) + 1);
    });
    -- column is a WOMB call (womb/ui/layout/flex.vuma)
    return womb_ui_layout_flex(
        FlexDirection::Column,
        Spacing::Fixed(8),
        [label_node, button_node]
    );
}
```

The VEEE `column` keyword doesn't do any layout work itself — it just
calls `womb_ui_layout_flex` with `FlexDirection::Column`. All the actual
Flexbox math (measure, position, flex grow/shrink distribution) is in
WOMB, PMT-verified.

### 6.3 What the VEEE standard library does NOT contain

- **No layout algorithm implementations** — those are in WOMB.
- **No font parser, shaper, or BiDi algorithm** — those are in WOMB.
- **No renderer** — that's in WOMB.
- **No event loop** — that's in WOMB.
- **No capability model** — that's in VUMA (consumed by WOMB).
- **No crypto, net, or kernel code** — that's in WOMB (existing modules).

The VEEE standard library is intentionally thin (~2,000 lines of VEEE
source). If a VEEE program needs a feature that isn't in the standard
library, the answer is usually "that feature should be added to WOMB,
not VEEE."

---

## 7. GPU path (from ADR-0022)

VEEE's GPU path is **hand-written GLSL → glslangValidator → SPIR-V →
embed as const byte array → WOMB gpu_dispatch**. This is the path locked
in by ADR-0022 (Accepted), which supersedes the rejected ADR-0018 (MLIR)
approach.

### 7.1 The flow (end to end)

```
1. VEEE developer writes a GLSL shader (hand-written, not VEEE DSL):
   shaders/path_tessellate.comp.glsl (~500 LOC)

2. At build time, veeec's build.rs invokes glslangValidator:
   glslangValidator -V shaders/path_tessellate.comp.glsl -o shaders/path_tessellate.spv

3. spirv-val validates the SPIR-V (build-time, catches shader bugs early):
   spirv-val shaders/path_tessellate.spv

4. The VEEE source references the .spv via #[embed]:
   const TESSELLATE_PATH_SPIRV : [u8; 4096] = #[embed("shaders/path_tessellate.spv")]

5. veeec lowers #[embed] to a VUMA Lit::Bytes (requires V-26):
   const TESSELLATE_PATH_SPIRV : [u8; 4096] = [0x03, 0x02, 0x23, 0x07, ...]

6. VUMA's pipeline compiles the const byte array into .rodata:
   .rodata
   TESSELLATE_PATH_SPIRV:
       .byte 0x03, 0x02, 0x23, 0x07, ...

7. At runtime, WOMB's gpu_dispatch host import loads the SPIR-V:
   vk_create_compute_pipeline_spirv(device, &TESSELLATE_PATH_SPIRV, 4096);

8. The GPU executes the shader:
   vk_cmd_dispatch(cmd, 256, 1, 1);
```

### 7.2 What VEEE provides

- The `#[embed("file.spv")]` attribute — VEEE syntax that pulls in a
  pre-compiled SPIR-V file as a const byte array. (~100 LOC in
  `veeec/src/embed.rs`.)
- The `gpu_render_pass` UI primitive (in VEEE's standard library), which
  lowers to a call to WOMB's `gpu_dispatch`.
- The build script (`veeec/build.rs`, ~50 LOC) that invokes
  `glslangValidator` and `spirv-val` on `.glsl` files.

### 7.3 What VEEE does NOT provide (by design)

- **No VEEE GPU DSL.** The shader is hand-written GLSL, not VEEE. A VEEE
  GPU DSL would be more ergonomic (type-checked, integrated with VEEE's
  incremental computation), but the SWE package's plan is GLSL + build
  script, and that's sufficient for v0.1. A VEEE GPU DSL can be added
  later (lowering to GLSL source, not MLIR) if needed. (ADR-0022
  §"Neutral".)
- **No runtime shader compilation.** Shaders load as fast as any const
  byte array (memcpy from `.rodata`). Runtime shader compilation would
  be slow (50-200ms per shader) and lose compile-time validation.
- **No PMT verification of GPU kernels.** PMT is a CPU memory model. GPU
  kernels use a different memory model (workgroup-shared memory, global
  memory, image memory). Verifying GPU kernels would require a new Lean
  model, which is out of scope for v1. (ADR-0022 §"Negative".)
  Mitigation: `spirv-val` runs at build time, catching shader bugs
  before runtime. Runtime spirv-cross validation (on Metal/WebGPU paths)
  catches platform-specific issues.

### 7.4 What WOMB provides

- `womb/ui/render/gpu_dispatch.vuma` — the `gpu_dispatch` host import:
  ```vuma
  extern "C" {
      transform vk_create_compute_pipeline_spirv(
          device: Address, spirv: Address, spirv_len: i64
      ) -> i64;
      transform vk_cmd_bind_pipeline(cmd: Address, pipeline: i64) -> i32;
      transform vk_cmd_dispatch(cmd: Address, x: i32, y: i32, z: i32) -> i32;
  }
  ```
- `womb/ui/render/vector.vuma` — the path-tessellation renderer that
  calls `gpu_dispatch` with the embedded SPIR-V blob.

### 7.5 What the C host runtime provides

These are "Wrap" decisions per the SWE package's `16-build-vs-buy.md` —
OS-provided GPU APIs wrapped by a thin C shim. No Rust GPU crates.

- Vulkan linkage (`libvulkan.so` / `vulkan-1.dll` / MoltenVK on macOS).
- Metal linkage (`-framework Metal` on macOS, via MoltenVK or native).
- WebGPU linkage (`libwgpu_native.so` for browser-embedded use).

### 7.6 The V-26 dependency (the only VUMA-side patch)

The GPU path requires **V-26: Const byte arrays** — parser support for
`Lit::Bytes(Vec<u8>)` and `Expr::ArrayLit`. This is a small, well-scoped
VUMA-side patch (~2 weeks per the catalog). It's the **only** VUMA-side
patch the GPU path needs.

V-26 is also useful for non-GPU purposes (font subsetting, embedded
assets), so it has value beyond the GPU path. It's deferred because the
VUMA team is focused on the v1 ship (month 18); VEEE starts at month 18,
so V-26 lands sometime in months 18-26 (the VEEE development window).

---

## 8. Dependency on VUMA + WOMB

VEEE's features depend on VUMA-side fixes and WOMB-side modules. The
table below maps each VEEE feature to its dependencies and their status.

### 8.1 Dependency matrix

| VEEE feature | Required VUMA fix | Status | Required WOMB module | Status |
|---|---|---|---|---|
| `signal T` (i32, u64, bool) | V-34 (`bridge_type_to_ir_type` maps f32/f64) | **DONE** | `womb/ui/reactive/graph.vuma` (mark_dirty) | Greenfield |
| `signal T` (f32, f64) | V-34, V-36 (`StateRead`/`StateWrite` hardcoded to I64) | **DONE** (V-34), V-36 in progress | (as above) | Greenfield |
| `signal T` (struct, nested) | V-35 (`type_size_from_name` returns 8 for layouts), V-38 (`bridge_type_size` has `_ => 8` for Struct) | In progress | (as above) | Greenfield |
| `derive { ... }` | (none beyond signal) | — | `womb/ui/reactive/memo.vuma` (memoization cache) | Greenfield |
| `monotone set<T>` | V-03 (semantic/codegen SCG parity) — for `ensures` discharge | Deferred (v2) | `womb/collections/monotone_set.vuma` | Greenfield |
| `antitone set<T>` | (same as monotone) | Deferred (v2) | `womb/collections/monotone_set.vuma` (symmetric) | Greenfield |
| `render { ... }` | V-08 (LayoutNode with all fields) — deferred to WOMB per ADR-0013 | WOMB concern | `womb/ui/render/scene.vuma`, `womb/ui/render/scene_build.vuma` | Greenfield (W-5) |
| `schedule { ... }` | (none — schedule is a record value) | — | `womb/ui/render/scene.vuma` (RenderConfig struct) | Greenfield (W-5) |
| `ui text` | V-A2-3 (text shaping correctness) | Deferred | `womb/ui/text/shaper_v1.vuma` | Greenfield (W-2) |
| `ui button` | (none — button is a WOMB component) | — | `womb/ui/components/button.vuma` | Greenfield |
| `ui text_input` | V-11 (session types `Choice`/`Offer` for IME channel) | In progress | `womb/ui/ime/textfield.vuma`, `womb/ui/ime/composition.vuma` | Greenfield (W-6) |
| `gpu_render_pass` (GPU shaders) | V-26 (const byte arrays) | Deferred (~2 weeks) | `womb/ui/render/gpu_dispatch.vuma`, `womb/ui/render/vector.vuma` | Greenfield (W-5) |
| `gpu_render_pass` (CPU fallback) | (none — CPU rasterizer is pure VUMA) | — | `womb/ui/render/cpu_raster.vuma` (WebGL2 fragment-shader path) | Greenfield (W-5) |
| `veee::animation::tween` | V-14 runtime (f32 `__float_overflow_trap`, exit 142) | DONE (v1) | `womb/ui/animation.vuma` | Greenfield (W-4) |
| `veee::a11y::semantics` | (none — a11y is a WOMB concern) | — | `womb/ui/a11y/semantics.vuma`, `womb/ui/a11y/build.vuma` | Greenfield (W-7) |
| `veee::theme::manager` | (none — theme is a WOMB concern) | — | `womb/ui/theme.vuma` | Greenfield (W-4) |

### 8.2 Critical path

The critical path for VEEE v0.1 is:

1. **VUMA v1 ships** (month 18): V-34, V-35, V-36, V-38 (bridge fixes);
   V-11 (session types); V-14 runtime (f32 trap); V-16 (HMAC-SHA256
   capability); V-26 (const byte arrays — the only VUMA-side patch
   needed after v1).
2. **WOMB v1 ships** (month 18, parallel with VUMA): W-0 (event
   pipeline), W-1 (font parser), W-2 (text shaper), W-3 (BiDi), W-4
   (layout engine), W-5 (vector renderer), W-6 (IME), W-7 (a11y), W-8
   (net/clipboard/file), W-9 (capability model).
3. **VEEE E-0** (months 18-20): language design — syntax, type system,
   reactivity model, algorithm/schedule separation.
4. **VEEE E-1** (months 20-24): compiler — lexer, parser, type checker,
   reactivity analyzer, VUMA AST lowering, e-graph rewrite rules.
5. **VEEE E-2** (months 24-26): standard library — core components,
   style system, animation DSL, state management, event handlers.
6. **VEEE v0.1 ships** (month 26): production-ready UX language.

### 8.3 What VEEE cannot ship without

VEEE v0.1 **cannot ship** until all of the following are true:

- VUMA v1 is shipped (all V-0..V-5 phases complete).
- WOMB v1 is shipped (all W-0..W-9 phases complete).
- V-26 (const byte arrays) is shipped (for the GPU path).
- `womb/ui/reactive/graph.vuma` is shipped (for `signal`/`derive`).
- `womb/collections/monotone_set.vuma` is shipped (for
  `monotone set`/`antitone set`).
- `womb/ui/render/scene.vuma` and `womb/ui/render/scene_build.vuma` are
  shipped (for `render`).
- `womb/ui/text/shaper_v1.vuma` is shipped (for `ui text`).
- `womb/ui/render/gpu_dispatch.vuma` is shipped (for `gpu_render_pass`).

If any of these slip, VEEE v0.1 slips. There is no "minimum viable VEEE"
without them — VEEE's value proposition (verified UI with incremental
computation, monotonicity types, algorithm/schedule separation, GPU
rendering) requires all of these to be in place.

---

## 9. Timeline

The VEEE timeline is **months 18-26** (per ADR-0013 and the SWE package's
`26-new-plans-three-layers.md`). VEEE starts after VUMA v1 + WOMB v1
ship.

### 9.1 Phase breakdown

| Phase | Months | Duration | Deliverable |
|---|---|---|---|
| **VUMA v1** | 1-18 | 18 months | Compiler + verification + 19 backends stable. (Parallel with WOMB from month 3.) |
| **WOMB v1** | 3-18 | 16 months | UI engine libraries (layout, render, text, IME, a11y, event, animation, theme). (Parallel with VUMA.) |
| **VEEE E-0: Language design** | 18-20 | 2 months | VEEE language spec: syntax, type system, reactivity model, algorithm/schedule separation. |
| **VEEE E-1: Compiler** | 20-24 | 4 months | `veeec` binary: lexer, parser, type checker, reactivity analyzer, VUMA AST lowering, e-graph rewrite rules. |
| **VEEE E-2: Standard library** | 24-26 | 2 months | VEEE stdlib: layout, text, render, reactive, collections, animation, theme, a11y, event. |
| **VEEE v0.1** | 26 | — | Production-ready UX language. |
| **VEEE E-3: Tooling** (post-v0.1) | 26-28 | 2 months | LSP, hot reload, DevTools, structured editor. |

### 9.2 VEEE E-0 (months 18-20): Language design

| Component | Effort |
|---|---|
| Syntax design (incremental-first, structured-editing-friendly) | 2 weeks |
| Type system design (signals, monotonicity types, session types) | 3 weeks |
| Reactivity model (Salsa-style demand-driven incremental queries) | 3 weeks |
| Algorithm/schedule separation (Halide-inspired) | 2 weeks |
| **Total E-0** | **10 weeks** |

**Deliverable**: VEEE language specification document (~100 pages),
with formal grammar, type system rules, and lowering sketches.

### 9.3 VEEE E-1 (months 20-24): Compiler

| Component | Effort | Estimated LOC |
|---|---|---|
| Lexer (hand-written NFA) | 2 weeks | ~400 |
| Parser (recursive descent) | 2 weeks | ~800 |
| Type checker (signals, monotonicity, effects) | 4 weeks | ~1,500 |
| Reactivity analyzer (dependency graph, invalidation) | 4 weeks | ~600 |
| VUMA AST lowering (VEEE → VUMA) | 4 weeks | ~800 |
| E-graph optimization (VEEE-specific rewrite rules) | 2 weeks | ~200 |
| Code generation (VEEE component → VUMA transforms) | 3 weeks | (included in lower.rs) |
| `#[embed]` attribute handler | 1 week | ~100 |
| Build script (`glslangValidator` invocation) | 1 day | ~50 |
| **Total E-1** | **~21 weeks** | **~5,000 LOC Rust** |

**Deliverable**: `veeec` binary that takes `.veee` source → VUMA AST →
VUMA codegen → Wasm (or native). Full PMT verification applies to
generated code.

### 9.4 VEEE E-2 (months 24-26): Standard library

| Component | Effort |
|---|---|
| Core components (Text, Button, Column, Row, ScrollView, TextField) | 3 weeks |
| Style system (styles as record values, no CSS) | 2 weeks |
| Animation DSL (`animate(from, to, 1s, ease_in)`) | 2 weeks |
| State management (signals, derived state, effects) | 2 weeks |
| Event handlers (pure state transitions, not imperative) | 1 week |
| **Total E-2** | **10 weeks** |

**Deliverable**: VEEE standard library (~2,000 lines of VEEE source)
that wraps WOMB UI primitives. The five example programs in §4.5
(counter, todo list, text label, animation, GPU shader dispatch)
compile and run.

### 9.5 VEEE v0.1 (month 26): Ship

**Definition of done** for VEEE v0.1:

1. The five example programs in §4.5 compile and run on at least one
   backend (wasm32 for browser, x86_64 for native).
2. The compiler produces PMT-verified binaries (no `--no-verify`).
3. The dev-build experience (`veeec --dev`) compiles a typical VEEE
   program in under 2 seconds.
4. The production-build experience (`veeec --release`) compiles a
   typical VEEE program with full optimization (e-graph, LTO, PGO) and
   produces a binary that runs at 60 FPS for the animation example.
5. The GPU path (`gpu_render_pass`) works on at least one GPU API
   (Vulkan on Linux recommended first).
6. The VEEE standard library is complete enough that the five example
   programs use only stdlib + WOMB (no ad-hoc VEEE code).

### 9.6 VEEE E-3 (months 26-28, post-v0.1): Tooling

| Component | Effort |
|---|---|
| LSP (language server protocol) | 4 weeks |
| Hot reload (recompile VEEE → swap Wasm, preserve state) | 4 weeks |
| DevTools (element inspector, layout debugger, profiler) | 6 weeks |
| Structured editor (projectional editing for component trees) | 8 weeks |
| **Total E-3** | **22 weeks** |

**Deliverable**: VEEE tooling ecosystem. Post-v0.1; not blocking VEEE
v0.1 ship.

### 9.7 Team allocation

Per the SWE package's `26-new-plans-three-layers.md`:

- **Engineer 6 (VEEE compiler)**: E-0 (10w) → E-1 (21w) → E-2 (10w).
- **Engineer 7 (VEEE tooling)**: (wait for E-1, 21w) → E-3 (22w).

VEEE v0.1 ships at month 26 with 2 engineers. (The VUMA and WOMB teams
are separate, 3-5 engineers each, months 1-18.)

---

## 10. Open questions

These are the questions that remain open for VEEE v0.2+ and beyond. They
don't block VEEE v0.1, but they should be resolved before v0.2.

### Q-VEEE-01: Should VEEE have its own type checker, or reuse VUMA's?

**Context**: ADR-0014 says "VEEE's type system must be encodable as VUMA
contracts" — implying VEEE reuses VUMA's type checker via contracts. But
ADR-0017 says "VEEE's type checker is non-trivial (~3,000 lines of Rust
to implement monotonicity inference, delta evaluation, and seminaïve
fixpoint)" — implying VEEE has its own type checker.

**Resolution for v0.1**: VEEE has its own type checker for
VEEE-specific features (signals, monotonicity, derive). For
VUMA-inherited features (i32, f32, struct, transform, channel,
capability), VEEE defers to VUMA's type checker (post-lowering). The
VEEE type checker runs *before* lowering; VUMA's type checker runs
*after* lowering. They check different properties at different
abstraction levels.

**Open for v0.2+**: Should VEEE's type checker and VUMA's type checker
share infrastructure (e.g., a common effect-inference module)? Or
should they stay fully separate, with VEEE's checker being a pure
pre-pass? The former would reduce duplication; the latter would keep
the layers cleanly separated. Deferred to v0.2.

### Q-VEEE-02: Should VEEE support metaprogramming (macros)?

**Context**: The SWE package says VEEE is "structured-editing-friendly"
but doesn't specify whether VEEE supports macros. Hazel-style live
editing is compatible with macros (macros are just AST-to-AST
transforms), but adding macros increases compiler complexity.

**Options**:
1. **No macros** (v0.1). VEEE's standard library is hand-written; users
   can't extend it. Simplest.
2. **Hygienic macros** (v0.2+). Like Rust's `macro_rules!`. Allows users
   to extend the standard library.
3. **Full compile-time metaprogramming** (v0.3+). Like Zig's comptime or
   Lisp macros. Most powerful, most complex.

**Recommendation for v0.1**: Option 1 (no macros). VEEE v0.1 ships
without macros. If users need patterns that the standard library
doesn't cover, they write VUMA directly (VEEE programs can call VUMA
transforms — see Q-VEEE-03).

### Q-VEEE-03: How does VEEE handle interop with existing VUMA code?

**Context**: Can a VEEE program call a VUMA transform directly? E.g.,
can `counter.veee` call `womb/ui/layout/flex.vuma`'s
`womb_ui_layout_flex` transform?

**Options**:
1. **Direct interop** (v0.1): VEEE programs can `import` VUMA modules
   and call VUMA transforms directly. The VEEE type checker treats VUMA
   transforms as opaque functions with VUMA types.
2. **FFI layer** (v0.2+): VEEE programs call VUMA through a typed FFI
   layer (like Rust's `extern "C"`). More ceremony, but better type
   safety.
3. **No interop** (rejected): VEEE programs can't call VUMA directly;
   everything must go through the VEEE standard library. Too
   restrictive — VEEE can't anticipate every WOMB API.

**Recommendation for v0.1**: Option 1 (direct interop). VEEE's standard
library is a thin wrapper over WOMB; users who need a feature not in
the stdlib can call WOMB directly. The VEEE type checker should produce
a clear error if the VUMA transform's types don't match VEEE's
expectations.

### Q-VEEE-04: Should VEEE's GPU DSL be a separate language or embedded in VEEE?

**Context**: ADR-0022 defers the VEEE GPU DSL to v0.2+ (v0.1 uses
hand-written GLSL). When the DSL arrives, should it be:
1. **Embedded in VEEE** (a `gpu kernel` block in `.veee` files, lowering
   to GLSL source).
2. **A separate language** (`.veee-gpu` files, compiled by a separate
   `veee-gpu` compiler).

**Recommendation for v0.2+**: Option 1 (embedded). A `gpu kernel` block
in `.veee` files, lowering to GLSL source, then to SPIR-V via
`glslangValidator` (the same build-time tool). This keeps the GPU path
consistent with ADR-0022 (no MLIR, no LLVM) and lets VEEE's type
checker verify the kernel's interface (buffer types, workgroup size).

### Q-VEEE-05: What's the VEEE LSP story?

**Context**: VUMA has an LSP module (per the SWE package). Should VEEE
extend VUMA's LSP, or have its own?

**Options**:
1. **Extend VUMA's LSP**: VEEE LSP is a thin layer over VUMA's LSP,
   adding VEEE-specific features (signal dependency visualization,
   monotonicity type display, derive-graph navigation).
2. **Separate VEEE LSP**: VEEE has its own LSP (`veee-lsp`), independent
   of VUMA's. More work, but cleaner separation.
3. **Hybrid**: VEEE LSP for VEEE source; defers to VUMA LSP for
   generated VUMA source (e.g., when debugging lowered code).

**Recommendation for v0.1**: Option 2 (separate `veee-lsp`). VEEE's LSP
needs VEEE-specific features (dependency graph visualization, monotone
type display) that don't belong in VUMA's LSP. The hybrid (Option 3)
can come in v0.2+ for debugging lowered code.

### Q-VEEE-06: Hot reload — preserve which state?

**Context**: VEEE E-3 (post-v0.1) includes hot reload (recompile VEEE →
swap Wasm, preserve state). But "preserve state" is ambiguous:
- Preserve all `signal` values?
- Preserve only `signal` values marked `#[persist]`?
- Preserve the PMT arena (everything)?

**Recommendation for v0.2+**: Preserve all `signal` values by default;
allow `#[ephemeral]` annotation for signals that should reset on hot
reload (e.g. animation phase). The PMT arena is too coarse (it includes
scene-tree scratch space that should be rebuilt).

### Q-VEEE-07: Should VEEE support concurrent signals (multi-threaded reactivity)?

**Context**: VUMA's V-15 (concurrent PMT) is deferred to v2 (per
`19-open-questions.md` Q-01). VEEE v0.1 is single-threaded (the UI
thread is the only thread). But VEEE's incremental-computation model
could in principle support concurrent signal updates (different signals
on different threads).

**Recommendation**: Defer to v0.3+ (after VUMA's V-15 concurrent PMT
ships). VEEE v0.1 is single-threaded; this matches VUMA v1's
single-threaded PMT. Concurrent VEEE is a v0.3+ feature, gated on V-15.

### Q-VEEE-08: Should VEEE's `schedule` block support autotuning?

**Context**: §4.3 mentions that Halide-style schedule autotuning is
deferred to v0.2+. The question is *how* autotuning should work:
1. **Build-time autotuning**: the VEEE compiler explores the schedule
   space at build time, benchmarks each option, picks the best.
2. **Runtime autotuning**: the WOMB renderer benchmarks schedule options
   on the first few frames, picks the best.
3. **Manual**: no autotuning; the user specifies the schedule explicitly.

**Recommendation for v0.2+**: Option 1 (build-time autotuning), gated
on a `--autotune` flag. The VEEE compiler generates multiple schedule
variants, runs them on a benchmark, picks the best per target. This is
the Halide approach. Option 2 (runtime) is too complex for v0.2.

### Q-VEEE-09: What's the VEEE story for testing?

**Context**: VUMA has `tests/gold_standard/` (benchmark programs) and
`tests/backend_latency_tests.rs` (per-ISA latency tables). VEEE needs
analogous testing infrastructure.

**Recommendation for v0.1**:
- `veee/tests/gold_standard/` — VEEE versions of the VUMA gold-standard
  programs (counter, todo list, text label, animation, GPU shader
  dispatch).
- `veee/tests/snapshot/` — snapshot tests for the VEEE → VUMA AST
  lowering (compare generated AST against a checked-in snapshot).
- `veee/tests/verification/` — tests that VEEE programs pass PMT
  verification (no `--no-verify`).
- `veee/tests/egraph/` — tests for the VEEE-specific e-graph rewrite
  rules (`derive_constant_fold`, `signal_dead_write_elim`, etc.).

### Q-VEEE-10: Should VEEE have a formal semantics?

**Context**: VUMA has a formal semantics in Lean (`proof/PMT/`). Should
VEEE have one too?

**Recommendation**: VEEE v0.1 doesn't need a formal semantics — VEEE
programs inherit VUMA's PMT verification, so the formal guarantee
comes from VUMA. A VEEE formal semantics (in Lean or Coq) would be
useful for proving VEEE-specific properties (e.g. "monotone collections
only grow") that VUMA's PMT doesn't cover. Deferred to v0.3+ (it's a
research project, not a v0.1 blocker).

---

## 11. References

### ADRs (binding decisions)

- **ADR-0010** — 5-crate external dependency policy (VEEE preserves it:
  zero new Rust crates; glslangValidator is a build-time tool, not a
  crate).
- **ADR-0012** — VEEE name adoption (VEEE = Verified Expression
  Evaluation Engine; .veee extension; veeec binary; veee repo).
- **ADR-0013** — Three-layer architecture (VUMA / WOMB / VEEE; VEEE is
  Layer 3; timeline months 18-26).
- **ADR-0014** — VEEE compiles to VUMA AST, not IR (VEEE inherits full
  PMT verification; estimated ~5,000 lines of Rust for the VEEE
  compiler).
- **ADR-0015** — SUPERSEDED by ADR-0023 (Cranelift/MLIR approach
  rejected; VUMA's hand-write philosophy prevails).
- **ADR-0016** — Incremental computation engine lives in VEEE, not VUMA
  (signal lowers to State<T> + mark_dirty; VUMA stays minimal).
- **ADR-0017** — Monotonicity types are a VEEE-layer type-system feature
  (monotone/antitone sets lower to requires/ensures contracts Z3
  discharges).
- **ADR-0018** — SUPERSEDED by ADR-0022 (MLIR GPU approach rejected;
  hand-written GLSL + glslangValidator prevails).
- **ADR-0022** — Hand-written SPIR-V backend (GLSL → glslangValidator →
  SPIR-V → embed as const byte array via V-26; no MLIR, no LLVM).
- **ADR-0023** — VEEE dev builds use VUMA codegen with `--dev` flags, not
  Cranelift (no new Rust crates; verification stays ON in dev).
- **ADR-0024** — Promote ADR-0007 to Accepted (IVE l1l3_collapse wiring
  verified; capability model is compile-time-only).
- **ADR-0025** — Extend SIMD coverage incrementally (add ops as
  text-shaper benchmarks demand them; affects VEEE production-build
  performance).

### VUMA-side source (verified by K-1)

- `src/codegen/src/egraph.rs` — 3235 lines, `pub enum ENode` :81,
  `pub struct EGraph` :141, PMT state-operation ENodes +
  `state_store_load_forward` rewrite rule (the exact rule VEEE needs for
  cheap signal-change reactivity).
- `src/codegen/src/bv_verify.rs` — bitvector verification
  (proof-carrying e-graph rewrite rules).
- `src/codegen/src/proof_artifacts.rs` — proof artifact emission for
  verified rewrites.
- `src/codegen/src/backend.rs:784` — `pub enum BackendKind` with 19
  variants (all CPU ISAs; no GPU — VUMA codegen is CPU-only by design).
- `src/ive/Cargo.toml:17-22` — Z3 is a HARD dependency: "The 'V' in
  VUMA depends on Z3."
- `src/ive/src/verification.rs` — `discharge_contracts_and_prove_blocks`
  (Z3 contract discharge; this is what discharges VEEE's monotonicity
  `ensures` contracts).
- `src/codegen/src/marshal.rs:66-79` — `ArgMode::Borrow/Marshal/MayRetain/ForeignPass/Invalidate`
  (the typed-state API VEEE's `signal` lowers to).
- `src/codegen/src/runtime/arena.rs:68` — `pub struct Arena` (the PMT
  arena `___pmt_buffer`).
- `src/codegen/src/ipc.rs:521` — `Resource::Channel(u64)`; `:1222`
  linear single-use windows (session-typed channels VEEE inherits).
- `src/codegen/src/capability.rs:117` — hardcoded `b"vuma_dev_signing_key"`
  (V-A3-2; ADR-0007 / ADR-0024 fix path).
- `src/parser/src/ast.rs` — VUMA AST (the compilation target for VEEE).
- `src/codegen/src/monomorphize.rs` — VUMA generics monomorphization
  (VEEE generics desugar to VUMA generics, monomorphized here).
- `proof/PMT/` — 82 Lean files including `Iris/` (separation logic),
  `BitVecArena.lean`, `PipelineSim.lean`, `WellTypedStrong.lean`,
  `RawArena.lean`, 22-file `Faithful/` directory. The Lean
  `pmt_soundness` theorem covers VEEE-lowered code (because it goes
  through the same SCG → IR path as hand-written VUMA).

### WOMB-side source (existing, PMT-verified VUMA code)

- `womb/crypto/mac_kdf/hmac.vuma` — RFC 2104 HMAC-SHA-256 (193 LOC;
  what ADR-0007 / ADR-0024 wires into VUMA's capability model).
- `womb/kernel/trap/irq_ring.vuma` — SPSC event ring (472 LOC; the
  pattern VEEE's W-0 event pipeline generalizes).
- `womb/collections/{vec,hashmap,btree_map}.vuma` — foundational data
  structures (VEEE's monotone set builds on these).
- `womb/lib/text/unicode.vuma` — RFC 3629 UTF-8 (709 LOC; used by VEEE's
  text primitives).
- `womb/lib/text/json.vuma` — RFC 8259 JSON (1254 LOC; used by VEEE's
  theme manager).

### WOMB-side greenfield (to be built, months 3-18)

- `womb/ui/reactive/graph.vuma` — dependency-graph data structure
  (consumed by VEEE's `mark_dirty` calls).
- `womb/ui/reactive/memo.vuma` — memoization cache (consumed by VEEE's
  `derive` blocks).
- `womb/collections/monotone_set.vuma` — monotone set data structure
  (consumed by VEEE's `monotone set<T>`).
- `womb/ui/render/scene.vuma`, `scene_build.vuma` — scene tree
  (consumed by VEEE's `render` block).
- `womb/ui/render/vector.vuma` — vector renderer (consumed by VEEE's
  `schedule { tessellation: gpu_compute, ... }`).
- `womb/ui/render/gpu_dispatch.vuma` — GPU host import (consumed by
  VEEE's `gpu_render_pass`).
- `womb/ui/text/shaper_v1.vuma` — text shaper (consumed by VEEE's
  `text` primitive).
- `womb/ui/layout/flex.vuma` — Flexbox (consumed by VEEE's `column` /
  `row`).
- `womb/ui/ime/textfield.vuma`, `composition.vuma` — IME (consumed by
  VEEE's `text_input`).
- `womb/ui/a11y/semantics.vuma`, `build.vuma` — accessibility (consumed
  by VEEE's `veee::a11y::semantics`).

### SWE package files (the research ground)

- `vuma-swe-package/22-review-against-vuma.md` — the audit that
  introduced Vell → VUMA (Part 5 introduced the higher-level language
  layer; VELL is renamed to VEEE per ADR-0012).
- `vuma-swe-package/23-vell-redesign-research.md` — the VELL redesign
  rationale: Salsa / Datafun / Halide / e-graph / structured editing /
  Cranelift+MLIR backend strategy. (Note: the Cranelift+MLIR backend
  strategy is superseded by ADR-0023 + ADR-0022.)
- `vuma-swe-package/26-new-plans-three-layers.md` — the authoritative
  three-layer plan: VUMA (months 1-18) + WOMB (months 3-18) + VEEE
  (months 18-26). Phase E-0/E-1/E-2/E-3 breakdown for VEEE.
- `vuma-swe-package/19-open-questions.md` — open questions from the SWE
  package (Q-01 concurrent PMT, Q-02 SIMD, etc. — most are decided or
  P3-deferred; VEEE-specific open questions are in §10 of this draft).
- `vuma-swe-package/16-build-vs-buy.md` — "Build (Python script)" decision
  for SPIR-V build tooling; "Pure VUMA kernel, no Rust crates in kernel"
  (the policy ADR-0022 enforces).

### VUMA-side research drafts

- `docs/research/K-1-veee-rename-design.md` — the full VEEE rename +
  three-layer architecture design report (1018 lines; the spiritual
  ancestor of this fine draft). 7 ADRs identified for Wave L; 5
  High-confidence (Accepted), 2 Medium-confidence (Proposed, later
  superseded by ADR-0022 + ADR-0023).
- `docs/research/J-1-womb-layer.md` — WOMB layer audit (what exists in
  `womb/`, what's missing for WOMB v1; the greenfield `womb/ui/`
  directory VEEE depends on).
- `docs/vuma-side-research-draft-v2.md` — Wave F corrected VUMA-side
  draft (WOMB and VEEE explicitly out of scope; K-1 is the first formal
  introduction of WOMB and VEEE into the ADR series).

### External research cited (from `23-vell-redesign-research.md` §7)

- **Salsa**: https://github.com/salsa-rs/salsa — Rust incremental
  computation framework (rust-analyzer).
- **Adapton**: http://adapton.org/ — the theoretical foundation for
  demand-driven incremental computation.
- **Datafun**: https://www.rntz.net/files/datafun.pdf — functional
  Datalog with monotonicity types (ICFP 2016).
- **Seminaïve Datafun**:
  https://www.cl.cam.ac.uk/~nk480/seminaïve-datafun.pdf — seminaïve
  evaluation for Datafun (2020).
- **Halide**: https://halide-lang.org/ — Adobe Research + MIT,
  algorithm/schedule separation for image processing pipelines.
- **egg**: https://github.com/egraphs-good/egg — Rust e-graph library
  (VUMA doesn't use it; VUMA hand-writes its own e-graph).
- **Cranelift**: https://github.com/bytecodealliance/cranelift — Rust
  codegen (REJECTED for VEEE per ADR-0023; would be 6th external dep).
- **MLIR**: https://mlir.llvm.org/ — multi-level IR (REJECTED for VEEE
  per ADR-0022; C++ toolchain violates hand-write philosophy).
- **MLIR SPIR-V dialect**: https://mlir.llvm.org/docs/Dialects/SPIR-V/
  (REJECTED; glslangValidator is sufficient for hand-written GLSL
  shaders).
- **Hazel**: https://hazel.org/ — live programming with typed holes
  (the structured-editing inspiration for VEEE's syntax).
- **Laminar**: https://laminar.dev/virtual-dom — FRP without virtual
  DOM (the conceptual cousin of VEEE's incremental-computation model).

### VUMA catalog entries (status as of 2026-08-01)

- **V-26** (const byte arrays) — Deferred (~2 weeks; the only VUMA-side
  patch VEEE's GPU path needs).
- **V-34** (`bridge_type_to_ir_type` f32/f64 mapping) — DONE.
- **V-35** (`type_size_from_name` for layouts) — In progress.
- **V-36** (`StateRead`/`StateWrite` hardcoded to I64) — In progress.
- **V-38** (`bridge_type_size` for Struct) — In progress.
- **V-03** (semantic/codegen SCG parity) — Deferred (v2; affects
  monotonicity `ensures` discharge).
- **V-11** (session types `Choice`/`Offer` for IME) — In progress.
- **V-14** (f32 PMT Lean proof) — Deferred to v2; v1 uses runtime
  `__float_overflow_trap` (exit 142).
- **V-15** (concurrent PMT) — Deferred to v2 (gates VEEE concurrent
  signals, Q-VEEE-07).
- **V-16** (HMAC-SHA256 capability signature) — In progress (ADR-0007
  promoted to Accepted by ADR-0024).

---

**End of VEEE Layer Fine Draft.**

This is the **final engineering plan** for the VEEE UX language layer.
All design decisions are locked by ADR-0012 through ADR-0017 and
ADR-0022 through ADR-0023. The five example programs in §4.5 demonstrate
the language in action. The open questions in §10 are deferred to VEEE
v0.2+ and don't block v0.1 ship at month 26.

The next actions for the VEEE team (starting month 18):
1. Begin VEEE E-0 (language design) — syntax, type system, reactivity
   model, algorithm/schedule separation (10 weeks).
2. Track VUMA-side V-26 (const byte arrays) — the only VUMA-side patch
   VEEE needs post-v1.
3. Track WOMB-side greenfield modules (`womb/ui/reactive/`,
   `womb/collections/monotone_set.vuma`, `womb/ui/render/scene.vuma`,
   etc.) — these are VEEE's runtime dependencies.
4. Prototype the VEEE → VUMA AST lowering on the counter example (§4.5
   Example 1) — this validates the integration point
   (`vuma::pipeline::compile_modules`) before E-1 starts in earnest.
