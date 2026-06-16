# Relational Descriptors (RelD) — Formal Mathematical Specification

**VUMA Project — Behavioral Descriptors Component**
**Task ID:** W1-04
**Date:** 2026-03-04
**Status:** Specification Draft v1.0

---

## 1. Relation Types

### 1.1 Syntactic Definition

Relational Descriptors (RelD) form the third component of the Behavioral Descriptor triple `(RepD, CapD, RelD)`. While RepD governs physical memory layout and CapD governs permissible operations, RelD governs the *relationships* that values bear to one another — temporal ordering, structural containment, data- and control-flow dependency, semantic equivalence, information-security flow, and liveness scoping. These six relation families are exhaustive in the sense that every inter-value relationship relevant to program correctness can be decomposed into a combination of these primitives.

We begin by fixing a universe of discourse. Let `ValId` denote the countable set of value identifiers — unique, stable names assigned by the Inference and Verification Engine (IVE) to every value node in the Semantic Computation Graph (SCG). Let `Path` denote finite sequences of field selectors. Let `ScopeId` denote the countable set of lexical and dynamic scope identifiers. Let `SecLevel = {Public, Internal, Confidential, Secret, TopSecret}` denote the linear security lattice ordered by `Public ≤ Internal ≤ Confidential ≤ Secret ≤ TopSecret`. We write `RepMapping` for the set of partial functions `ValId ⇀ ValId` that map fields of one representation onto fields of another (the formal underpinning of zero-copy reinterpretation).

The grammar of relations is:

```
Relation ::= TemporalRel { kind: TemporalKind, target: ValId }
           | ContainmentRel { container: ValId, field: Path }
           | DependencyRel { source: ValId, kind: DepKind }
           | EquivalenceRel { equivalent: ValId, mapping: RepMapping }
           | SecurityRel { level: SecLevel, flow: FlowPolicy }
           | LivenessRel { scope: ScopeId, phase: Phase }

TemporalKind ::= Outlives | Coincides | Precedes | Succeeds
DepKind      ::= DataDep | ControlDep | AliasDep
FlowPolicy   ::= NoDowngrade | NoCrossBoundary | Sanitized
Phase        ::= Init | Steady | Teardown | Dropped
```

A RelD is the aggregation of all relations pertaining to a single value:

```
RelD ::= RelD { relations: Set<Relation> }
```

### 1.2 Semantic Interpretation

Each relation is a constraint on the trace semantics of the program. Let `T = (E, ≤_E, σ)` be a trace: a partially ordered set of events `E` with happens-before ordering `≤_E` and store mapping `σ : ValId × E ⇀ ByteVec`. The satisfaction relation `T ⊨ r` (trace `T` satisfies relation `r`) is defined case-by-case:

- **TemporalRel{Outlives, y}**: For the value `v` carrying this relation and target `y`, `T ⊨ TemporalRel{Outlives, y}` iff the maximal event at which `σ(v, ·)` is defined is ≥ in the happens-before order than the maximal event at which `σ(y, ·)` is defined. Informally: `v` is live at least as long as `y`.

- **TemporalRel{Coincides, y}**: `T ⊨ TemporalRel{Coincides, y}` iff `v` and `y` share exactly the same liveness interval: the minimal and maximal events of `v` and `y` in the trace are identical. Coincides is strictly stronger than mutual Outlives.

- **TemporalRel{Precedes, y}**: The maximal event of `v` is ≤ the minimal event of `y`. Value `v` completes before `y` begins.

- **TemporalRel{Succeeds, y}**: The minimal event of `v` is ≥ the maximal event of `y`. Value `v` begins only after `y` completes.

- **ContainmentRel{c, p}**: Value `v` is a sub-field of container `c` at path `p`. Formally, for every event `e` where `σ(v, e)` is defined, `σ(c, e)` is also defined and the byte range of `v` is a subset of the byte range of `c`.

- **DependencyRel{s, DataDep}**: Every event that reads `v` is preceded by an event that wrote `s`. The data-flow edge from `s` to `v` must be respected.

- **DependencyRel{s, ControlDep}**: Whether `v` is computed at all depends on a predicate involving `s`. This is a control-flow constraint, not a data-flow constraint: `v`'s existence in the trace is conditioned on `s`.

- **DependencyRel{s, AliasDep}**: `v` and `s` are derived from the same allocation root. A write through one may affect reads through the other. AliasDep is the relational analogue of Rust's aliasing analysis.

- **EquivalenceRel{e, m}**: `v` and `e` denote the same abstract entity, differing only in representation. The mapping `m : ValId ⇀ ValId` specifies which logical fields correspond. For each corresponding pair `(f_v, f_e)` in `m`, the bytes of `v` at `f_v` and the bytes of `e` at `f_e` encode semantically identical information (possibly in different byte orders, encodings, or layouts).

- **SecurityRel{ℓ, NoDowngrade}**: The security level of `v` is at least `ℓ`, and no operation on `v` may produce a value at a level below `ℓ`. This is the non-interference property: `T ⊨ SecurityRel{ℓ, NoDowngrade}` iff for every pair of traces `T₁, T₂` that agree on all events at level ≥ ℓ, the observable behavior of `v` is identical.

- **SecurityRel{ℓ, NoCrossBoundary}**: `v` at level `ℓ` must not cross a specified trust boundary (e.g., from a trusted enclave to untrusted host). Formally, there is no event that reads `v` and writes to a value whose SecurityRel carries a level below `ℓ`.

- **SecurityRel{ℓ, Sanitized}**: `v` originated at level `ℓ` but has been transformed by a sanctioned sanitization function, producing a value that may be treated at a lower level. The trace must contain a specific sanitization event between the origin of `v` and any downstream use at a reduced level.

- **LivenessRel{s, p}**: Value `v` is live exactly during phase `p` of scope `s`. The liveness interval of `v` is bounded by the phase transitions of `s`.

### 1.3 Well-Formedness

Not every set of relations is meaningful. A RelD `d = RelD{R}` is **well-formed** iff:

1. **No self-relation**: For no `r ∈ R` does `r` relate `v` to itself (trivial relations add no information and indicate a probable inference error).

2. **Temporal consistency within d**: If `TemporalRel{Outlives, y} ∈ R` and `TemporalRel{Precedes, y} ∈ R`, then `d` is ill-formed (a value cannot both outlive and precede the same target — the two constraints are contradictory unless `y` has zero liveness, which we exclude by requiring positive liveness intervals).

3. **Security monotonicity**: If `SecurityRel{ℓ₁, f₁} ∈ R` and `SecurityRel{ℓ₂, f₂} ∈ R` with `ℓ₁ ≠ ℓ₂`, then `ℓ₁` and `ℓ₂` must be compatible — i.e., the effective security level is `max(ℓ₁, ℓ₂)` with the most restrictive flow policy. This is the join operation of the security lattice.

We write `wf(d)` to denote that `d` is well-formed.

---

## 2. RelD Refinement

### 2.1 Motivation and Intuition

Refinement is the ordering relation on RelDs that captures the intuition "more restrictive / more informative / narrower." If `r₁ refines r₂`, then every trace that satisfies `r₁` also satisfies `r₂`, but not necessarily vice versa. Refinement is the relational analogue of subtyping: a value with a more refined RelD can be used wherever a less refined one is expected, because it carries stronger guarantees. This section defines `refines` precisely, proves it is a partial order, and establishes the key lattice properties that enable composition.

### 2.2 Pointwise Refinement of Relations

We first define refinement on individual relations, then lift to RelDs. The pointwise refinement relation `⊑` on `Relation` is defined as follows:

**Temporal refinement lattice.** The temporal kinds form a refinement lattice induced by logical strength:

```
Outlives(x, y) ⊑ Coincides(x, y)     [if x and y are the same pair]
Precedes(x, y) ⊑ Coincides(x, y)      [if x and y are the same pair]
Succeeds(x, y) ⊑ Coincides(x, y)      [if x and y are the same pair]
```

The justification: `Outlives(x, y)` asserts that `x`'s liveness extends at least to the end of `y`'s liveness — it is a *lower bound* on `x`'s lifetime. `Coincides(x, y)` asserts exact equality of liveness intervals — a *tighter* constraint. Any trace where `x` and `y` coincide certainly satisfies `Outlives`, but not vice versa. Similarly, `Precedes` and `Succeeds` each constrain one endpoint of the liveness interval, while `Coincides` constrains both. Formally:

```
{T | T ⊨ Coincides(x, y)} ⊆ {T | T ⊨ Outlives(x, y)}
```

Moreover, `Outlives(x, y) ∧ Outlives(y, x) ⟺ Coincides(x, y)`. This is the join: if both directions of Outlives hold, the intervals must be identical. We capture this as:

```
⊤_Temporal = Coincides   (most restrictive)
⊥_Temporal = Outlives    (least restrictive, for same direction)
```

Note that `Precedes(x, y)` and `Outlives(x, y)` are *incomparable* when considered as constraints on a single value `x` relative to `y` — Precedes constrains the end of `x` to precede the start of `y`, while Outlives constrains the end of `x` to follow the end of `y`. They occupy orthogonal axes of the temporal space. They are comparable only through Coincides, which dominates both.

**Security refinement.** Security relations refine according to the security lattice and flow-policy ordering:

```
SecurityRel{ℓ₁, NoDowngrade} ⊑ SecurityRel{ℓ₂, NoDowngrade}  iff  ℓ₁ ≥ ℓ₂
SecurityRel{ℓ, NoDowngrade}  ⊑ SecurityRel{ℓ, NoCrossBoundary}
SecurityRel{ℓ, NoCrossBoundary} ⊑ SecurityRel{ℓ, Sanitized}
```

The first rule: a higher security level is more restrictive. Any value at level `Secret` with `NoDowngrade` certainly satisfies the constraint for level `Internal` with `NoDowngrade` — the higher level subsumes the lower. The second rule: `NoDowngrade` is stricter than `NoCrossBoundary` because the former prohibits any flow downward while the latter only prohibits flows across trust boundaries. The third rule: `NoCrossBoundary` is stricter than `Sanitized` because the latter permits downgrade after approved sanitization.

**Containment refinement.** Containment relations refine by narrowing the path scope:

```
ContainmentRel{c, p₁} ⊑ ContainmentRel{c, p₂}  iff  p₂ is a prefix of p₁
```

If `p₁ = p₂.q`, then `v` is contained at a *deeper* field within `c`. A deeper containment is a narrower constraint: it asserts more about the location of `v` within `c`. Any trace satisfying the deeper containment also satisfies the shallower one, because the byte range of a deeper field is a subset of the byte range of a shallower field.

**Dependency refinement.** Dependency relations refine by increasing specificity:

```
DependencyRel{s, AliasDep} ⊑ DependencyRel{s, DataDep}
```

If two values are aliased, they are certainly data-dependent (a write through one observable through the other). The reverse does not hold. `ControlDep` and `DataDep` are incomparable — one constrains existence, the other constrains value.

**Equivalence refinement.** Equivalence relations refine by expanding the mapping:

```
EquivalenceRel{e, m₁} ⊑ EquivalenceRel{e, m₂}  iff  dom(m₁) ⊇ dom(m₂)
```

A larger mapping (more fields mapped) is a weaker constraint because it asserts correspondence over more fields. Wait — we must be careful. Actually, a *smaller* mapping is *less informative* and therefore *less restrictive*. A larger mapping `m₁ ⊇ m₂` says "not only do these fields correspond, but also these additional fields," which is a stronger assertion. Thus:

```
EquivalenceRel{e, m₁} ⊑ EquivalenceRel{e, m₂}  iff  m₁ ⊇ m₂  (m₁ maps more fields)
```

**Liveness refinement.** Liveness relations refine by narrowing the phase:

```
LivenessRel{s, Init} ⊑ LivenessRel{s, Steady}    iff  true   (Init ⊂ Steady in scope)
```

More precisely, `Init` is a sub-phase of `Steady` in the sense that any value live during `Init` is also live during the broader `Steady` phase. The refinement goes the other direction: `LivenessRel{s, Init}` is more restrictive (narrower liveness window), so it refines the broader constraint.

### 2.3 Lifting to RelD

Let `d₁ = RelD{R₁}` and `d₂ = RelD{R₂}`. We define:

```
d₁ refines d₂  (written d₁ ⊑ d₂)  iff
  ∀ r₂ ∈ R₂. ∃ r₁ ∈ R₁. r₁ ⊑ r₂ ∧ sameKind(r₁, r₂) ∧ sameTarget(r₁, r₂)
```

That is, every relation in `d₂` is "covered" by a more restrictive relation of the same kind targeting the same value in `d₁`. The refining RelD must be at least as strong along every dimension that the coarser RelD constrains.

### 2.4 Partial Order Proof

**Theorem 1.** The relation `⊑` on well-formed RelDs is a partial order (reflexive, antisymmetric, transitive).

*Proof.*

**Reflexivity.** For any well-formed `d = RelD{R}`, we have `d ⊑ d` because for each `r ∈ R`, we can choose `r₁ = r₂ = r` and `r ⊑ r` holds by the reflexivity of pointwise refinement (each pointwise rule is reflexive by construction).

**Antisymmetry.** Suppose `d₁ ⊑ d₂` and `d₂ ⊑ d₁`. Then every relation in `R₂` is covered by a more restrictive relation in `R₁`, and vice versa. By the antisymmetry of each pointwise refinement rule (each rule defines a partial order on relations of the same kind and target), the covering relations must be equal. Hence `R₁ = R₂` and `d₁ = d₂`.

**Transitivity.** Suppose `d₁ ⊑ d₂` and `d₂ ⊑ d₃`. For any `r₃ ∈ R₃`, by `d₂ ⊑ d₃` there exists `r₂ ∈ R₂` with `r₂ ⊑ r₃`. By `d₁ ⊑ d₂`, there exists `r₁ ∈ R₁` with `r₁ ⊑ r₂`. By transitivity of pointwise refinement, `r₁ ⊑ r₃`. Hence `d₁ ⊑ d₃`. ∎

### 2.5 Join and Meet

The refinement partial order admits joins and meets for compatible RelDs. Two RelDs are *compatible* if they constrain the same value. The **join** `d₁ ⊔ d₂` is the least upper bound: the weakest RelD that is refined by both. The **meet** `d₁ ⊓ d₂` is the greatest lower bound: the strongest RelD that refines both.

```
d₁ ⊔ d₂ = RelD{ r₁ ⊔ r₂ | r₁ ∈ R₁, r₂ ∈ R₂, sameKind(r₁,r₂), sameTarget(r₁,r₂) }
           ∪ (R₁ \ matched_in_R₂) ∪ (R₂ \ matched_in_R₁)
```

The join takes the weaker of each matched pair and preserves unmatched relations from both sides. This corresponds to the union of constraints, where shared constraints are weakened to their common generalization. The meet is defined dually, taking the stronger of each matched pair. Both operations preserve well-formedness given well-formed inputs.

**Lemma 1 (Join-Soundness).** If `wf(d₁)` and `wf(d₂)`, then `wf(d₁ ⊔ d₂)`.

*Proof sketch.* The join weakens constraints, so it cannot introduce contradictions. Any temporal inconsistency present in `d₁ ⊔ d₂` would have to arise from a matched pair `r₁, r₂` where `r₁ ⊔ r₂` is contradictory. But the pointwise join operations on each relation kind are defined to produce consistent results (e.g., `Outlives ⊔ Precedes` with same target is undefined, so the pair is treated as unmatched, preserving both). Unmatched relations from `d₁` and `d₂` are individually consistent by hypothesis. ∎

---

## 3. RelD Composition Across Operations

### 3.1 Overview

Programs are built from composition: function calls, assignments, branches, loops, and concurrent forks. Each composition operation must respect and propagate RelDs. This section defines how RelDs combine under each operation, establishing the composition laws that the IVE enforces. The guiding principle is that composition must preserve the soundness guarantee: if every individual RelD is satisfied before composition, and the composition law is respected, then every RelD is satisfied after composition.

### 3.2 Function Call

Consider a function `f` with formal parameter `p` and callee-body value `v`, called with actual argument `a`. Let `RelD(a)` be the RelD of the actual argument and `RelD(p)` be the RelD expected by the callee. The call is well-typed iff:

```
RelD(a) ⊑ RelD(p)     [argument refines parameter]
```

This is the relational analogue of contravariant subtyping on function arguments. The callee promises to respect the constraints in `RelD(p)`; the caller must provide a value that satisfies at least those constraints. If `RelD(a) ⊑ RelD(p)`, then every trace satisfying `RelD(a)` also satisfies `RelD(p)`, so the callee's assumptions hold.

For the return value `r`, the caller acquires:

```
RelD(r_caller) = RelD(r_callee)[v_return/caller_v]
```

where `[v_return/caller_v]` is the substitution that replaces callee-internal value identifiers with the caller's corresponding identifiers (the standard scope exit substitution). Additionally, any relations in `RelD(r_callee)` that reference values not escaping the callee are dropped, and any relations referencing values that do escape (e.g., through returned pointers) are re-targeted to the caller's namespace.

**Lemma 2 (Call-Soundness).** If `RelD(a) ⊑ RelD(p)` and the callee body preserves its own RelD constraints (induction hypothesis), then the caller's RelD after the call is consistent.

*Proof sketch.* By the refinement condition, the argument satisfies all callee assumptions. By the callee's internal soundness (inductive hypothesis), all callee-internal relations are satisfied. The substitution preserves satisfaction because it is a syntactic renaming that does not alter the trace semantics. Dropping non-escaping relations is sound because they constrain only callee-internal values that are no longer accessible from the caller. ∎

### 3.3 Assignment

For an assignment `x := e`, where `RelD(e) = RelD_e` is the RelD of the expression result and `RelD(x)_old` is the prior RelD of `x`:

```
RelD(x)_new = RelD_e[target ← x] ⊓ RelD(x)_old
```

The target `x` acquires the RelD of the source expression (with value identifiers re-targeted to `x`), *meet*-combined with any pre-existing constraints on `x` that survive the assignment. The meet is necessary because pre-existing relations may impose constraints that are not present in the source — for example, `x` may have a `SecurityRel{Confidential, NoDowngrade}` that persists across the assignment because the *location* `x` occupies is in a confidential-labeled memory region, regardless of what value is stored there.

**Scoping rule.** Temporal relations in `RelD_e` are adjusted: if `e`'s value has `TemporalRel{Outlives, y}` where `y` is a value in the current scope, the assignment to `x` extends `x`'s liveness to at least the point of assignment. The assigned-to `x` may then have a shorter liveness than `y`, in which case the `Outlives` relation is dropped and a warning is emitted (the value in `e` is constrained to outlive `y`, but `x` may not, creating a potential dangling relation).

### 3.4 Branch (Conditional)

For a conditional `if c then e₁ else e₂`, where both branches produce a value bound to `x`:

```
RelD(x) = RelD(e₁) ⊔ RelD(e₂)
```

The result's RelD is the **join** of the two branch RelDs. This is the weakest RelD that is refined by both branches, capturing the fact that after the conditional, the RelD must account for either path having been taken. This is the relational analogue of union types / meet types in type theory.

**Requirement.** Both branches must satisfy any *required* RelD `RelD_req` imposed by the continuation:

```
RelD(e₁) ⊑ RelD_req    and    RelD(e₂) ⊑ RelD_req
```

If one branch fails this check, the conditional is rejected. This ensures that the continuation's assumptions hold regardless of which branch is taken.

**Lemma 3 (Branch-Soundness).** If `RelD(e₁) ⊑ RelD_req` and `RelD(e₂) ⊑ RelD_req`, then `RelD(e₁) ⊔ RelD(e₂) ⊑ RelD_req`.

*Proof.* By definition of join, `d₁ ⊔ d₂` is the least upper bound, so `d₁ ⊑ d₁ ⊔ d₂` and `d₂ ⊑ d₁ ⊔ d₂`. Since `d₁ ⊑ RelD_req` and `d₂ ⊑ RelD_req`, and `d₁ ⊔ d₂` is the least upper bound, we have `d₁ ⊔ d₂ ⊑ RelD_req`. ∎

### 3.5 Loop

For a loop `while c do body`, the RelD of any value `v` modified in the loop body must satisfy the **loop invariant** condition:

```
RelD(v)_entry ⊑ RelD(v)_exit_body
```

where `RelD(v)_entry` is the RelD of `v` at the top of the loop and `RelD(v)_exit_body` is the RelD of `v` after one iteration of the body. This ensures that the RelD is *monotone with respect to refinement* across iterations: each iteration produces a RelD that is at least as strong as the one it started with. If the RelD were to weaken, the invariant would be violated.

Equivalently, the loop invariant `RelD_inv(v)` must satisfy:

```
RelD_inv(v) ⊑ RelD(v)_before_body   [pre-condition]
RelD(v)_after_body ⊑ RelD_inv(v)    [post-condition]
```

The first condition says the invariant is established before the first iteration. The second says each iteration restores the invariant. Together, they guarantee that the invariant holds at every loop head.

**Fixed-point formulation.** Let `F : RelD → RelD` be the transfer function of the loop body (mapping the RelD at entry to the RelD at exit). The loop invariant is a fixed point of `F`:

```
RelD_inv = ⊔{ F^n(⊥) | n ≥ 0 }
```

where `⊥` is the minimal (empty) RelD and `F^n` denotes `n`-fold application. Since the refinement lattice has finite height (bounded by the number of distinct relations, which is bounded by the size of the program), this iteration reaches a fixed point in finite steps.

**Explicit relaxation.** The programmer (or IVE) may specify a weaker invariant `RelD_inv' ≺ RelD_inv`, where `RelD_inv' ⊑ RelD_inv`. This is explicit relaxation: the loop is verified against the weaker invariant, which may be sufficient for the continuation's requirements. This is the relational analogue of loop-carried dependency relaxation in vectorization analysis.

**Lemma 4 (Loop-Termination-of-Analysis).** The fixed-point iteration `⊥, F(⊥), F²(⊥), …` terminates in at most `|R|` steps, where `R` is the set of all possible relations in the program.

*Proof.* Each application of `F` either produces a strictly more refined RelD (adding or strengthening at least one relation) or reaches a fixed point. The refinement ordering has finite height bounded by `|R|` (each step must add or strengthen at least one of finitely many possible relations). By the ascending chain condition, the sequence stabilizes in at most `|R|` steps. ∎

### 3.6 Concurrent Composition

For concurrent composition `e₁ || e₂`, the RelD of shared values must satisfy **non-interference**:

```
∀ v shared between e₁ and e₂:
  RelD(v)_e₁ ⊓ RelD(v)_e₂ is well-formed
```

and the security relations must not allow a write in one thread to violate the security constraints observed by the other. Formally:

```
∄ SecurityRel{ℓ, f} ∈ RelD(v)_e₁,  ∄ write(v) ∈ e₂
  where level(e₂) < ℓ
```

This prevents a low-security thread from observing high-security data through a shared mutable reference — the relational formulation of information-flow control for concurrency.

---

## 4. RelD Consistency

### 4.1 Definition

A set of RelDs `D = {d₁, …, dₙ}` is **consistent** iff there exists at least one trace `T` that satisfies all of them simultaneously:

```
consistent(D)  iff  ∃ T. ∀ dᵢ ∈ D. T ⊨ dᵢ
```

In practice, the IVE does not enumerate traces (the space is infinite). Instead, it checks a set of syntactic conditions that are *necessary* and *sufficient* for consistency given the well-formedness constraints of Section 1.3. This section defines those conditions and proves their equivalence to the semantic definition.

### 4.2 Consistency Conditions

**C1: No circular Outlives.** The Outlives relation induces a directed graph on values. Define:

```
G_outlives(D) = (V, E)  where
  V = { v | TemporalRel{Outlives, v} ∈ relations(dᵢ) for some dᵢ ∈ D }
  E = { (v, u) | TemporalRel{Outlives, u} ∈ RelD(v) }
```

**Condition C1:** `G_outlives(D)` must be acyclic.

**Justification.** If `v₁ outlives v₂ outlives … outlives v₁`, then by transitivity `v₁` must outlive itself, which means `v₁`'s liveness interval strictly exceeds itself — a contradiction for any finite trace.

**Exception.** A cycle `v outlives u ∧ u outlives v` is permitted iff `Coincides(v, u)` is also present. In this case, the cycle collapses to a single equivalence class of coincident values.

**C2: No security downgrades.** Define the security flow graph:

```
G_sec(D) = (V, E)  where
  V = all values with SecurityRel in D
  E = { (v, u) | ∃ data dependency from v to u,
         level(v) > level(u),
         flow_policy(v) ∈ {NoDowngrade, NoCrossBoundary} }
```

**Condition C2:** `G_sec(D)` must be empty (no edges).

**Justification.** An edge `(v, u)` with `level(v) > level(u)` and `NoDowngrade` on `v` means information flows from a higher-security value to a lower-security one, violating non-interference. The only exception is `Sanitized` flow, which requires a verified sanitization event on the path from `v` to `u`.

Formally, if `SecurityRel{ℓ, Sanitized} ∈ RelD(v)` and there is a sanitization function `sanitize : ValId → ValId` that the IVE has verified as reducing the security level, then the edge is permitted iff the trace contains `sanitize(v)` on the path from `v` to `u`.

**C3: No containment cycles.** Define the containment graph:

```
G_contain(D) = (V, E)  where
  V = all values with ContainmentRel in D
  E = { (v, c) | ContainmentRel{c, p} ∈ RelD(v) }
```

**Condition C3:** `G_contain(D)` must be acyclic.

**Justification.** If `v` is contained in `c` and `c` is contained in `v`, then the byte range of `v` is a strict subset of the byte range of `c` and vice versa — a contradiction unless `v = c` and `p` is the empty path, which is excluded by the no-self-relation well-formedness rule.

**C4: Temporal-Containment agreement.** If `v` is contained in `c`, then `v` must not outlive `c`:

```
ContainmentRel{c, p} ∈ RelD(v)  ⟹  TemporalRel{Outlives, c} ∉ RelD(v)
```

More precisely, the liveness interval of `v` must be a subset of the liveness interval of `c`. If `v` is contained in `c` but outlives `c`, then after `c` is deallocated, `v` would reference freed memory — a use-after-free in relational clothing.

**C5: Equivalence consistency.** If `EquivalenceRel{e, m} ∈ RelD(v)`, then the security levels must agree across the mapping:

```
∀ (fᵥ, fₑ) ∈ m:
  SecurityRel{ℓᵥ, _} ∈ RelD(v.fᵥ) ∧ SecurityRel{ℓₑ, _} ∈ RelD(e.fₑ)
  ⟹  ℓᵥ = ℓₑ
```

Semantically equivalent fields must carry the same security level, otherwise the equivalence mapping would create an implicit downgrade channel.

### 4.3 Formal Consistency Predicate

Combining C1–C5:

```
consistent(D) ⟺ wf_all(D)
                ∧ acyclic(G_outlives(D) \ coincides_edges(D))
                ∧ G_sec(D) = ∅
                ∧ acyclic(G_contain(D))
                ∧ C4_holds(D)
                ∧ C5_holds(D)
```

where `wf_all(D)` asserts that every `dᵢ ∈ D` is well-formed, and `coincides_edges(D)` removes edges `(v, u)` from the Outlives graph where `Coincides(v, u)` is also present.

### 4.4 Soundness and Completeness

**Theorem 2 (Soundness).** If `consistent(D)`, then there exists a trace `T` satisfying all `dᵢ ∈ D`.

*Proof sketch.* We construct `T` as follows. By C1, the Outlives graph (minus coincidences) is a DAG, so we can topologically sort values from longest-lived to shortest-lived. Assign each value a liveness interval consistent with this ordering and with all temporal relations. By C3, containment is a forest, so we assign byte ranges consistent with the containment hierarchy. By C2, there are no forbidden security flows, so we assign security levels consistent with the security lattice. By C4, contained values do not outlive their containers. By C5, equivalent fields have consistent security. The resulting trace satisfies all relations by construction. ∎

**Theorem 3 (Completeness).** If `¬consistent(D)`, then no trace `T` satisfies all `dᵢ ∈ D`.

*Proof.* By case analysis on the violated condition:
- If C1 is violated, the Outlives cycle implies a value must outlive itself strictly, which is impossible in any finite trace.
- If C2 is violated, the security downgrade violates non-interference, which is a trace-level property.
- If C3 is violated, the containment cycle implies a value strictly contains itself, which is impossible for finite byte ranges.
- If C4 is violated, a contained value outlives its container, implying a use-after-free.
- If C5 is violated, equivalent fields have different security levels, creating an implicit downgrade. ∎

### 4.5 Computational Complexity

Checking `consistent(D)` reduces to: (1) cycle detection in a directed graph (C1, C3), which is `O(|V| + |E|)`; (2) edge-existence checks in the security flow graph (C2), which is `O(|E_sec|)`; and (3) pairwise compatibility checks (C4, C5), which are `O(|R|)` where `R` is the total number of relations. Overall, consistency checking is **linear** in the size of the relation set, making it tractable for the IVE to check incrementally as the program is edited.

---

## 5. RelD Inference Algorithm Sketch

### 5.1 Overview

The IVE must infer RelDs for every value in the SCG without programmer annotation. This section sketches the inference algorithm, proves its termination, and argues its soundness relative to the consistency conditions of Section 4. The algorithm is a fixed-point computation over the SCG, driven by five propagation phases: initialization, data-flow propagation, temporal propagation, security propagation, and consistency checking.

### 5.2 Algorithm

**Input:** A Semantic Computation Graph `G = (N, E)` where `N` is the set of value nodes and `E` is the set of data-flow edges, along with scope annotations `scope : N → ScopeId` and allocation metadata `alloc : N → AllocInfo`.

**Output:** A mapping `ρ : N → RelD` assigning a RelD to every value node, such that `consistent({ρ(n) | n ∈ N})`.

**Phase 0: Initialization.** Set `ρ₀(n) = RelD{∅}` for all `n ∈ N`. Every value starts with an empty (unconstrained) RelD.

**Phase 1: Data-flow propagation.** For each data-flow edge `e = (n₁, n₂) ∈ E`:

```
ρ(n₂) ← ρ(n₂) ∪ { DependencyRel{n₁, DataDep} }
```

If the edge represents an alias derivation (e.g., pointer arithmetic from `n₁` to `n₂`):

```
ρ(n₂) ← ρ(n₂) ∪ { DependencyRel{n₁, AliasDep} }
```

If the edge represents control flow (e.g., `n₂` is computed only if `n₁` satisfies a predicate):

```
ρ(n₂) ← ρ(n₂) ∪ { DependencyRel{n₁, ControlDep} }
```

This phase is iterated until no new DependencyRel relations are added. Since each edge is processed once (the dependency structure is fixed by the SCG), this phase terminates in `O(|E|)` steps.

**Phase 2: Containment propagation.** For each allocation `a` with sub-fields `f₁, …, fₖ` identified by the RepD:

```
ρ(fᵢ) ← ρ(fᵢ) ∪ { ContainmentRel{a, path(fᵢ)} }
```

This adds containment relations from the structure of RepDs. It terminates in `O(Σᵢ |fields(aᵢ)|)` where the sum is over all allocations.

**Phase 3: Temporal propagation.** For each value `n`, based on its scope and the scope of values it references:

```
if scope(n) = s and scope(m) = s' where s' ⊂ s:
    ρ(n) ← ρ(n) ∪ { TemporalRel{Outlives, m} }
if scope(n) = scope(m):
    ρ(n) ← ρ(n) ∪ { TemporalRel{Coincides, m} }
```

Additionally, for values whose lifetimes are determined by allocation/deallocation events:

```
if n is allocated before m in the SCG execution order:
    ρ(n) ← ρ(n) ∪ { TemporalRel{Precedes, m} }
```

This phase uses the scope nesting structure and the SCG's topological ordering to assign temporal relations. The number of scope pairs is bounded by `|N|²`, so this phase terminates in `O(|N|²)`.

**Phase 4: Security propagation (taint analysis).** Initialize security levels from explicit source annotations (e.g., values from network I/O are tainted `Public`, values from encrypted stores are `Confidential`). Propagate through data-flow edges:

```
if DependencyRel{n₁, DataDep} ∈ ρ(n₂):
    level(n₂) ← level(n₂) ⊔ level(n₁)   [join in security lattice]
```

and add the corresponding security relation:

```
ρ(n₂) ← ρ(n₂) ∪ { SecurityRel{level(n₂), NoDowngrade} }
```

For values that cross trust boundaries, add `NoCrossBoundary`. For values that pass through verified sanitization functions, downgrade to `Sanitized`.

This is a standard taint analysis formulated as a dataflow problem on the SCG. Since the security lattice has finite height (5 levels), the propagation reaches a fixed point in at most `5 × |N|` steps.

**Phase 5: Consistency check.** Apply `consistent({ρ(n) | n ∈ N})` as defined in Section 4. If consistent, output `ρ`. If inconsistent:

1. Identify the violated condition (C1–C5).
2. Report the specific values and relations causing the violation.
3. Suggest minimal relaxations (e.g., adding `Coincides` to break an Outlives cycle, inserting sanitization to fix a security downgrade, adjusting scope to fix a containment-liveness mismatch).
4. If the programmer approves a relaxation, update `ρ` and re-check.

### 5.3 Termination Proof

**Theorem 4 (Termination).** The inference algorithm terminates for any finite SCG.

*Proof.* The algorithm processes each phase sequentially. We show each phase terminates:

- **Phase 0** is constant-time: `O(|N|)`.
- **Phase 1** adds at most one DependencyRel per edge, and edges are fixed. Total additions: `|E|`. Termination in `|E|` steps.
- **Phase 2** adds at most one ContainmentRel per field, and fields are determined by the (finite) RepD structure. Termination in `Σᵢ |fields(aᵢ)|` steps.
- **Phase 3** adds at most `|N|²` temporal relations (one per ordered pair of values). Termination in `|N|²` steps.
- **Phase 4** performs taint propagation on a lattice of height 5. Each value's level can increase at most 5 times (from `Public` to `TopSecret`). Total increases: `5 × |N|`. Termination in `5 × |N|` steps.
- **Phase 5** is a single consistency check, which by Section 4.5 runs in `O(|R|)` where `|R| ≤ 6 × |N|²` (at most 6 relation types per pair).

Since each phase terminates and phases are sequential, the algorithm terminates. ∎

**Corollary (Monotone Growth).** The RelD mapping `ρ` grows monotonically throughout the algorithm: `ρ₀ ⊑ ρ₁ ⊑ ρ₂ ⊑ ρ₃ ⊑ ρ₄`. Each phase only adds relations, never removes them. The only "shrinking" step is the consistency check, which may suggest removals — but these require programmer approval and produce a new, manually-validated `ρ`.

### 5.4 Soundness

**Theorem 5 (Soundness of Inference).** If the algorithm outputs `ρ` (i.e., consistency check passes), then for every value `n` in the SCG, the inferred `ρ(n)` is satisfied by every trace of the program.

*Proof sketch.* By induction over the phases:

- **Phase 1** adds only DependencyRel relations that are direct reflections of the SCG's data-flow structure. By definition of the SCG, every trace respects the data-flow edges, so every trace satisfies these dependency relations.

- **Phase 2** adds ContainmentRel relations that are direct reflections of the RepD structure. By the RepD's definition (which describes the actual memory layout), these containment relations are satisfied by construction.

- **Phase 3** adds TemporalRel relations derived from scope nesting and allocation ordering. By the operational semantics of scopes, values in outer scopes outlive values in inner scopes, and values allocated earlier precede values allocated later. Thus every trace satisfies these temporal relations.

- **Phase 4** adds SecurityRel relations by taint propagation. By the soundness of taint analysis on a finite lattice (standard result from information-flow theory), the propagated levels are upper bounds on the actual information content of each value. Thus every trace satisfies the NoDowngrade constraints.

- **Phase 5** verifies global consistency, which by Theorem 2 guarantees the existence of a satisfying trace.

The key insight: each phase adds only relations that are *justified* by the program structure, and the consistency check ensures they are mutually compatible. ∎

### 5.5 Precision and Completeness of Inference

The inferred RelDs are sound but not necessarily *complete* — there may exist additional relations that hold in all traces but are not inferred by the algorithm. For example:

- **Relational invariants**: If `x = y + 1` in all executions, the algorithm does not infer `EquivalenceRel` between `x` and `y` (they are not semantically equivalent; the relationship is arithmetic, not representational). This is by design: EquivalenceRel captures *representational* equivalence, not *computational* relationships.

- **Temporal refinements**: The algorithm infers `Outlives` from scope nesting but may not infer `Coincides` even when two values have identical liveness intervals in practice. This is because `Coincides` requires a stronger proof obligation. The IVE may refine `Outlives` to `Coincides` as part of its ongoing verification debt reduction.

**Lemma 5 (Maximality of DataDep Inference).** Phase 1 infers all data-dependency relations that are syntactically present in the SCG.

*Proof.* Every data-flow edge in the SCG is processed exactly once, and a DependencyRel{DataDep} is added for each. The SCG captures all data dependencies by construction (it is the canonical representation of the program's data flow). ∎

### 5.6 Incremental Re-Inference

When the SCG is modified (e.g., a node is added or an edge is changed), full re-inference is unnecessary. The algorithm supports incremental updates:

1. **Affected subgraph identification**: Determine which values are reachable from the modified node/edge via data-flow, containment, or temporal edges.
2. **Phase 1–4 re-execution** only on the affected subgraph.
3. **Global consistency re-check** (since local changes can have global implications, e.g., a new data-flow edge may create a security downgrade path).

This incremental approach reduces the average-case cost of re-inference from `O(|N|²)` to `O(|affected|² + |R|)` where `|affected|` is the size of the affected subgraph, typically much smaller than `|N|`.

---

## Appendix A: Notation Summary

| Symbol | Meaning |
|--------|---------|
| `ValId` | Countable set of value identifiers |
| `Path` | Finite sequences of field selectors |
| `ScopeId` | Countable set of scope identifiers |
| `SecLevel` | `{Public, Internal, Confidential, Secret, TopSecret}` |
| `RepMapping` | Partial functions `ValId ⇀ ValId` |
| `⊑` | Refinement (sub-relationship) ordering |
| `⊔`, `⊓` | Join and meet in refinement lattice |
| `T ⊨ r` | Trace `T` satisfies relation `r` |
| `wf(d)` | RelD `d` is well-formed |
| `consistent(D)` | Set of RelDs `D` is mutually consistent |
| `ρ : N → RelD` | Inferred RelD mapping |
| `G_outlives`, `G_sec`, `G_contain` | Consistency check graphs |

## Appendix B: Relation to Other BD Components

RelD interacts with RepD and CapD as follows:

- **RelD ↔ RepD**: ContainmentRel references the field structure defined by RepD. EquivalenceRel's RepMapping aligns fields across different RepDs. Changes to RepD may require re-inference of ContainmentRel and EquivalenceRel relations.

- **RelD ↔ CapD**: SecurityRel constrains which CapD capabilities may be exercised (e.g., `SecurityRel{Confidential, NoDowngrade}` prohibits the `send_over_network` capability unless `Sanitized` is also present). LivenessRel constrains when CapD capabilities are valid (e.g., `LivenessRel{s, Dropped}` invalidates all capabilities).

- **BD Integration**: A full BD `(RepD, CapD, RelD)` is well-formed iff each component is individually well-formed and the cross-component constraints (above) are satisfied. BD refinement is component-wise: `(r₁, c₁, d₁) ⊑ (r₂, c₂, d₂)` iff `r₁ ⊑ r₂ ∧ c₁ ⊑ c₂ ∧ d₁ ⊑ d₂`.

---

*End of RelD Formal Specification — Task W1-04*
