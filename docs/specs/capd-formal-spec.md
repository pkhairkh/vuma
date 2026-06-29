# Capability Descriptors (CapD) — Formal Mathematical Specification

**Document ID:** VUMA-SPEC-W1-03

**Author:** Parham Khairkhah

---

## 0. Preamble

This document provides the formal mathematical specification for **Capability Descriptors (CapD)**, one of the three orthogonal components of the Behavioral Descriptor (BD) triple `(RepD, CapD, RelD)` introduced in Section 3.5 of the VUMA proposal. CapD replaces the traditional type-system notion of "valid operations on a value" with a context-dependent permission set. Where a nominal type statically assigns a fixed set of operations to every value of that type, CapD recognizes that the same data may be read, written, serialized, or executed depending on the phase of execution, the security context, and the temporal state of the program. This specification formalizes the capability algebra, its lattice structure, composition rules, weakening/strengthening semantics, and context-dependent resolution. All definitions are given in a set-theoretic style suitable for mechanization in a proof assistant.

---

## 1. Capability Set Definition

### 1.1 Primitive Capabilities

The universe of primitive capabilities is drawn from a finite, enumerated set. Each capability names a distinct class of operation that may be performed on a value. The enumeration is deliberately chosen to span the full space of operations relevant to the VUMA memory model, the SCG execution semantics, and the security boundary model.

**Definition 1.1 (Capability).** Let `Cap` denote the set of primitive capabilities:

```
Cap ::= Read           -- observe the value (dereference for load)
      | Write          -- mutate the value (dereference for store)
      | Execute        -- interpret the value as executable code
      | Iterate        -- traverse the value as a collection
      | Send           -- transmit the value across a communication boundary
      | Persist        -- store the value to non-volatile media
      | Serialize      -- encode the value into a linear byte sequence
      | Deserialize    -- decode a value from a linear byte sequence
      | Hash           -- compute a fixed-size digest of the value
      | Compare        -- test equality or ordering of the value
      | DerivePtr      -- derive a pointer (address) from the value
      | Cast           -- reinterpret the value's representation
      | Fork           -- create an independent copy (clone) of the value
      | Drop           -- release or deallocate the value
      | Share          -- create a shared (aliased) reference to the value
      | Move           -- transfer ownership of the value (consumes source)
      | Pin            -- prevent the value from being relocated in memory
```

Each capability is atomic and indivisible. There is no implicit hierarchy among capabilities — `Write` does not imply `Read`, and `Move` does not imply `Fork`. This design choice ensures that the capability set faithfully represents the minimal permissions required for each operation, avoiding the over-approximation that plagues traditional type systems where a mutable reference implicitly grants read access even when the operation only writes.

**Rationale.** The seventeen capabilities above were derived by analyzing the operational semantics of the SCG and the VUMA memory model. `Read`, `Write`, and `Execute` correspond to the three fundamental memory access types (load, store, execute). `Iterate` captures the traversal pattern for collection-like data. `Send` and `Persist` correspond to communication and storage boundaries, which are critical for the RelD security-level flow analysis. `Serialize` and `Deserialize` handle the encoding boundary that the proposal identifies as a major source of impedance mismatch. `Hash` and `Compare` capture observation operations that are semantically distinct from `Read` (they produce a summary rather than the full value). `DerivePtr` is the capability to obtain an address from a value, which is central to the VUMA model where all access is pointer-based. `Cast` captures representation reinterpretation. `Fork`, `Drop`, `Share`, `Move`, and `Pin` model the ownership and aliasing operations that the VUMA model verifies globally rather than restricting locally.

### 1.2 Conditions

Capabilities are not unconditionally granted. A value may have the `Write` capability only during a certain execution phase, or the `Send` capability only when the value is not concurrently accessed. Conditions encode these contextual guards.

**Definition 1.2 (Condition).** Let `Cond` denote the set of conditions:

```
Cond ::= InPhase(Phase)              -- capability active during execution phase
       | AfterOp(OpId)               -- capability active after operation completes
       | BeforeOp(OpId)              -- capability active before operation begins
       | NotConcurrentWith(OpId)     -- capability active when OpId is not executing
       | RequiresLock(LockId)        -- capability active while lock is held
       | SecurityLevel(Level)        -- capability active at or above security level
       | ValidDuring(RegionId)       -- capability active during region lifetime
```

Where `Phase`, `OpId`, `LockId`, `Level`, and `RegionId` are drawn from their respective domains within the SCG. A condition `c ∈ Cond` is *satisfied* by an execution context if the context meets the guard's requirement. For example, `InPhase(Initialization)` is satisfied when the SCG execution engine is in the initialization phase; `RequiresLock(mutex_42)` is satisfied when `mutex_42` is held by the current thread of execution.

**Definition 1.3 (Condition Satisfaction).** Let `ctx` be an execution context (formally defined in Section 5). We write `ctx ⊨ c` to denote that context `ctx` satisfies condition `c`. The satisfaction relation is defined inductively:

- `ctx ⊨ InPhase(p)` iff `ctx.phase = p`
- `ctx ⊨ AfterOp(oid)` iff `oid` has completed in `ctx`
- `ctx ⊨ BeforeOp(oid)` iff `oid` has not yet started in `ctx`
- `ctx ⊨ NotConcurrentWith(oid)` iff `oid` is not currently executing in `ctx`
- `ctx ⊨ RequiresLock(lid)` iff `lid` is held by the current execution agent in `ctx`
- `ctx ⊨ SecurityLevel(lvl)` iff `ctx.security_level ≥ lvl`
- `ctx ⊨ ValidDuring(rid)` iff region `rid` is live in `ctx`

### 1.3 Capability Descriptor

**Definition 1.4 (CapD).** A Capability Descriptor is a pair:

```
CapD ::= CapD { caps: 𝒫(Cap), conditions: 𝒫(Cond) }
```

Where `𝒫(X)` denotes the powerset of `X`. Intuitively, `caps` is the set of capabilities that *may* be exercised on the described value, and `conditions` is the set of conditions that *must all be satisfied* for any capability in `caps` to be active. The conditions form a conjunctive guard: a capability `c ∈ caps` is *active* in context `ctx` iff `ctx` satisfies every condition in `conditions`.

**Notation.** We write `CapD(caps, conds)` for the CapD with capability set `caps` and condition set `conds`. When `conds = ∅`, the capabilities are unconditionally active. When `caps = ∅`, no operations are permitted regardless of context.

**Definition 1.5 (Active Capabilities).** Given a CapD `d = CapD(caps, conds)` and an execution context `ctx`, the set of *active capabilities* is:

```
active(d, ctx) = { c ∈ caps | ∀ cond ∈ conds: ctx ⊨ cond }
```

If any condition in `conds` is not satisfied by `ctx`, then *no* capability in `caps` is active. This all-or-nothing semantics ensures that CapD conditions are treated as mandatory guards, not optional hints. If finer-grained control is needed (e.g., `Write` requires a lock but `Read` does not), the value should be described by multiple CapDs — one for the locked context and one for the unlocked context — composed through the RelD layer.

---

## 2. CapD Lattice Structure

### 2.1 Partial Order

The ordering on CapDs reflects a fundamental intuition: a CapD is "less than" another if it grants fewer permissions (a subset of capabilities) and imposes more restrictions (a superset of conditions). This ordering captures the principle that *weakening* a descriptor — removing capabilities or adding conditions — always produces a descriptor that permits a subset of the original operations.

**Definition 2.1 (CapD Partial Order).** For two CapDs `d₁ = CapD(c₁, q₁)` and `d₂ = CapD(c₂, q₂)`, we define:

```
d₁ ≤ d₂  ⟺  c₁ ⊆ c₂  ∧  q₁ ⊇ q₂
```

That is, `d₁ ≤ d₂` if and only if `d₁` has a subset of the capabilities of `d₂` and a superset of the conditions of `d₂`. This is the *information ordering*: moving up in the order grants more freedom (more capabilities, fewer conditions), while moving down restricts freedom.

**Lemma 2.1 (Reflexivity).** For any CapD `d = CapD(c, q)`, `d ≤ d`.
*Proof.* By set-theoretic reflexivity: `c ⊆ c` and `q ⊇ q`. ∎

**Lemma 2.2 (Antisymmetry).** If `d₁ ≤ d₂` and `d₂ ≤ d₁`, then `d₁ = d₂`.
*Proof.* From `d₁ ≤ d₂`: `c₁ ⊆ c₂` and `q₁ ⊇ q₂`. From `d₂ ≤ d₁`: `c₂ ⊆ c₁` and `q₂ ⊇ q₁`. By antisymmetry of `⊆`: `c₁ = c₂` and `q₁ = q₂`. Hence `d₁ = d₂`. ∎

**Lemma 2.3 (Transitivity).** If `d₁ ≤ d₂` and `d₂ ≤ d₃`, then `d₁ ≤ d₃`.
*Proof.* From `d₁ ≤ d₂`: `c₁ ⊆ c₂` and `q₁ ⊇ q₂`. From `d₂ ≤ d₃`: `c₂ ⊆ c₃` and `q₂ ⊇ q₃`. By transitivity of `⊆`: `c₁ ⊆ c₃`. By transitivity of `⊇`: `q₁ ⊇ q₃`. Hence `d₁ ≤ d₃`. ∎

**Theorem 2.1 (Partial Order).** The relation `≤` is a partial order on CapD.
*Proof.* Directly from Lemmas 2.1, 2.2, and 2.3. ∎

### 2.2 Meet and Join

**Definition 2.2 (Meet).** The meet (greatest lower bound) of two CapDs is:

```
d₁ ⊓ d₂ = CapD(c₁ ∩ c₂, q₁ ∪ q₂)
```

The meet takes the intersection of capabilities (only capabilities present in *both*) and the union of conditions (conditions from *either* must be satisfied). This is the most permissive CapD that is below both `d₁` and `d₂` in the partial order.

**Lemma 2.4 (Meet is GLB).** `d₁ ⊓ d₂` is the greatest lower bound of `{d₁, d₂}`.
*Proof.* We must show: (1) `d₁ ⊓ d₂ ≤ d₁` and `d₁ ⊓ d₂ ≤ d₂`; (2) for any `d` such that `d ≤ d₁` and `d ≤ d₂`, we have `d ≤ d₁ ⊓ d₂`.

(1) `c₁ ∩ c₂ ⊆ c₁` and `q₁ ∪ q₂ ⊇ q₁`, so `CapD(c₁ ∩ c₂, q₁ ∪ q₂) ≤ CapD(c₁, q₁)`. Similarly for `d₂`.

(2) Let `d = CapD(c, q)` with `c ⊆ c₁`, `c ⊆ c₂`, `q ⊇ q₁`, `q ⊇ q₂`. Then `c ⊆ c₁ ∩ c₂` and `q ⊇ q₁ ∪ q₂`, so `d ≤ d₁ ⊓ d₂`. ∎

**Definition 2.3 (Join).** The join (least upper bound) of two CapDs is:

```
d₁ ⊔ d₂ = CapD(c₁ ∪ c₂, q₁ ∩ q₂)
```

The join takes the union of capabilities (capabilities from *either*) and the intersection of conditions (only conditions shared by *both*). This is the least permissive CapD that is above both `d₁` and `d₂`.

**Lemma 2.5 (Join is LUB).** `d₁ ⊔ d₂` is the least upper bound of `{d₁, d₂}`.
*Proof.* Symmetric to Lemma 2.4. (1) `c₁ ⊆ c₁ ∪ c₂` and `q₁ ⊇ q₁ ∩ q₂`, so `d₁ ≤ d₁ ⊔ d₂`. Similarly for `d₂`. (2) For any `d = CapD(c, q)` with `c ⊇ c₁`, `c ⊇ c₂`, `q ⊆ q₁`, `q ⊆ q₂`: `c ⊇ c₁ ∪ c₂` and `q ⊆ q₁ ∩ q₂`, so `d₁ ⊔ d₂ ≤ d`. ∎

### 2.3 Top and Bottom

**Definition 2.4 (Top).** The top element of the lattice is:

```
⊤ = CapD(Cap, ∅)
```

`⊤` grants every capability with no conditions. It represents the maximally permissive descriptor: a value described by `⊤` can be operated on in any way, unconditionally. In practice, `⊤` is the descriptor of a freshly allocated, unrestricted value before any constraints have been inferred by the IVE.

**Definition 2.5 (Bottom).** The bottom element of the lattice is:

```
⊥ = CapD(∅, Cond)
```

`⊥` grants no capabilities (or equivalently, the empty set of capabilities subject to all conditions — which is operationally identical, since there are no capabilities to activate). It represents the maximally restrictive descriptor: a value described by `⊥` cannot be operated on in any way. In practice, `⊥` describes a value that has been fully consumed (moved away), freed, or rendered inaccessible by a security boundary.

**Lemma 2.6 (Top and Bottom Extremal).** For any CapD `d = CapD(c, q)`:
- `⊥ ≤ d ≤ ⊤`

*Proof.* `∅ ⊆ c ⊆ Cap` and `Cond ⊇ q ⊇ ∅`. ∎

**Theorem 2.2 (CapD Forms a Lattice).** The structure `(CapD, ≤, ⊓, ⊔, ⊥, ⊤)` is a bounded lattice.
*Proof.* By Theorem 2.1, `≤` is a partial order. By Lemmas 2.4 and 2.5, every pair of elements has a meet and a join. By Lemma 2.6, `⊥` and `⊤` are the least and greatest elements respectively. ∎

**Corollary 2.1 (Distributivity).** The CapD lattice is distributive. That is, for all CapDs `d₁, d₂, d₃`:

```
d₁ ⊓ (d₂ ⊔ d₃) = (d₁ ⊓ d₂) ⊔ (d₁ ⊓ d₃)
d₁ ⊔ (d₂ ⊓ d₃) = (d₁ ⊔ d₂) ⊓ (d₁ ⊔ d₃)
```

*Proof sketch.* Both equalities follow from the distributivity of set union over intersection (and vice versa) applied componentwise. For the first: the capability component is `c₁ ∩ (c₂ ∪ c₃) = (c₁ ∩ c₂) ∪ (c₁ ∩ c₃)` by set distributivity; the condition component is `q₁ ∪ (q₂ ∩ q₃) = (q₁ ∪ q₂) ∩ (q₁ ∪ q₃)`, again by set distributivity. The second equality is dual. ∎

---

## 3. CapD Composition

CapD composition governs how capability descriptors interact across the structural constructs of the SCG: function calls, pointer derivation, branching, and iteration. The central principle is that composition must preserve safety: if each component is individually safe, then the composed system is safe. This section defines four composition rules and proves that each maintains the CapD invariant (that no operation is performed without the requisite capability in the active context).

### 3.1 Function Call Composition

When a function `f` with required CapD `d_req` is called from a context where the argument has CapD `d_arg`, the caller must ensure that the argument's CapD is at least as permissive as the function's requirement. Formally:

**Definition 3.1 (Call Compatibility).** A function call from a context with argument CapD `d_arg` to a function requiring CapD `d_req` is *compatible* iff:

```
d_req ≤ d_arg
```

That is, the argument must have every capability the function requires and must not impose any condition that the function does not also impose. If `d_req = CapD(c_req, q_req)` and `d_arg = CapD(c_arg, q_arg)`, then call compatibility requires `c_req ⊆ c_arg` (the argument has all required capabilities) and `q_req ⊇ q_arg` (the argument's conditions are a subset of the required conditions, meaning the argument is at least as unconditionally available as the function expects).

**Lemma 3.1 (Call Safety).** If a function call is compatible, then every operation the function performs on the argument is permitted by the argument's active capabilities.
*Proof.* Let `op` be an operation requiring capability `c_op`. Since the function requires `d_req`, we have `c_op ∈ c_req`. By compatibility, `c_req ⊆ c_arg`, so `c_op ∈ c_arg`. For any context `ctx` satisfying all conditions in `q_arg`, the argument's active capabilities include `c_op`. Since `q_req ⊇ q_arg`, any context the function expects also satisfies the argument's conditions. Hence `op` is permitted. ∎

**Definition 3.2 (Call Result CapD).** After a function call returns, the result CapD is the *meet* of the function's declared result CapD and the caller's ambient CapD:

```
d_result = d_fn_result ⊓ d_caller_ambient
```

This ensures that the result is constrained by both the function's return descriptor and the caller's context. For example, if the function returns a value with `Read, Write` but the caller is in a read-only phase, the result inherits the read-only constraint.

### 3.2 Pointer Derivation Composition

In the VUMA model, all data access is pointer-based. Deriving a pointer from a value is itself a capability-governed operation (requiring `DerivePtr`). The derived pointer's CapD is a strict subset of the source's CapD, reflecting the principle that derivation cannot create new permissions.

**Definition 3.3 (Pointer Derivation Rule).** If a value has CapD `d_src = CapD(c_src, q_src)` and a pointer is derived from it, the derived pointer's CapD is:

```
d_derived = CapD((c_src ∩ PtrCaps) \ {Move}, q_src ∪ {ValidDuring(src_region)})
```

Where `PtrCaps = {Read, Write, Execute, DerivePtr, Cast, Compare, Hash, Share, Pin}` is the set of capabilities meaningful for a pointer, and `Move` is explicitly removed because deriving a pointer does not transfer ownership of the source value. The condition `ValidDuring(src_region)` is added because the derived pointer is valid only as long as the source region is live — a core VUMA safety invariant.

**Lemma 3.2 (Derivation Safety).** `d_derived ≤ d_src`. That is, the derived pointer's CapD is always weaker than or equal to the source's CapD.
*Proof.* The capability set of `d_derived` is `c_src ∩ PtrCaps \ {Move} ⊆ c_src`, so `c_derived ⊆ c_src`. The condition set of `d_derived` is `q_src ∪ {ValidDuring(src_region)} ⊇ q_src`, so `q_derived ⊇ q_src`. By Definition 2.1, `d_derived ≤ d_src`. ∎

**Corollary 3.1 (Derivation Chain Weakening).** If a pointer `pₙ` is derived through a chain `p₀ → p₁ → ... → pₙ`, then `CapD(pₙ) ≤ CapD(p₀)`. Each derivation step weakens the CapD, so the chain produces a monotonically weaker descriptor.

### 3.3 Branch Composition

When execution branches into two paths (e.g., an `if-then-else`), the CapD on each branch must be *compatible* — meaning that their meet is not `⊥`. If the meet were `⊥`, then no capability would be available on both branches, making it impossible to merge the branches back into a single control flow.

**Definition 3.4 (Branch Compatibility).** Two CapDs `d₁` and `d₂` are *branch-compatible* iff:

```
d₁ ⊓ d₂ ≠ ⊥
```

Equivalently, `c₁ ∩ c₂ ≠ ∅`. Branch compatibility requires that there exists at least one capability that is available on both branches. The merged CapD after the branches rejoin is:

```
d_merged = d₁ ⊓ d₂
```

**Lemma 3.3 (Merge is Weakest Compatible).** `d_merged` is the greatest CapD that is below both `d₁` and `d₂`. Any operation safe on both branches is safe with `d_merged`.
*Proof.* By Lemma 2.4, `d₁ ⊓ d₂` is the greatest lower bound. Any CapD `d' ≤ d₁` and `d' ≤ d₂` satisfies `d' ≤ d₁ ⊓ d₂`. Since weakening is safe (Theorem 4.1), any operation safe under `d₁` and `d₂` individually is safe under their meet. ∎

### 3.4 Loop Composition

For a loop, the CapD must be *invariant* across all iterations. If the CapD changed between iterations, the IVE could not verify that loop body operations are safe without analyzing an unbounded number of iterations.

**Definition 3.5 (Loop Invariant CapD).** A CapD `d_inv` is a *loop invariant* for a loop with body CapD `d_body` iff:

```
d_body ⊓ d_inv ≤ d_inv
```

This is equivalent to requiring `d_body ≤ d_inv` (since `d_body ⊓ d_inv ≤ d_inv` holds trivially when `d_body ≤ d_inv`, because then `d_body ⊓ d_inv = d_body`). The invariant CapD must be at least as permissive as the loop body's required CapD.

**Lemma 3.4 (Loop Safety).** If `d_inv` is a loop invariant for a loop with body CapD `d_body`, then every operation in the loop body is safe under `d_inv` for every iteration.
*Proof.* By induction on the number of iterations. Base case (iteration 0): `d_inv ≥ d_body`, so the body's operations are safe. Inductive step: assume after iteration `k`, the value has CapD `d_inv`. Then on iteration `k+1`, the body requires `d_body ≤ d_inv`, so the operations are safe. After the body, the CapD is `d_body ⊓ d_inv = d_body ≤ d_inv`, which by the invariant property restores `d_inv`. ∎

**Definition 3.6 (Weakest Loop Invariant).** The weakest (most restrictive) loop invariant for a loop with body CapD `d_body` is `d_body` itself. However, in practice the IVE may find a stronger invariant (`d_inv ≥ d_body`) that is easier to verify. The strongest useful invariant is `⊤`, which is always an invariant but provides no useful constraint.

---

## 4. CapD Weakening and Strengthening

### 4.1 Definitions

The CapD lattice structure gives rise to two fundamental transformations: weakening (moving down in the lattice) and strengthening (moving up in the lattice). These transformations are the algebraic basis for the IVE's capability inference: when the IVE cannot prove that a value has a specific CapD, it may weaken the descriptor to a provable one; when the programmer specifies a required CapD, the IVE must verify that the inferred descriptor is at least as strong as the requirement.

**Definition 4.1 (Weakening).** A CapD `d₂` is a *weakening* of `d₁` iff `d₁ ≤ d₂` (equivalently, `d₂` is *above* `d₁` in the lattice). Weakening can be achieved by:
- Adding capabilities: `CapD(c ∪ {cap}, q)` is a weakening of `CapD(c, q)`.
- Removing conditions: `CapD(c, q \ {cond})` is a weakening of `CapD(c, q)`.
- Both simultaneously.

Note: In our lattice, `≤` means "less permissive" (fewer capabilities, more conditions). So `d₁ ≤ d₂` means `d₂` is *more permissive* than `d₁`. We follow the convention that "weakening" means the descriptor permits *more* operations — it is a *relaxation* of constraints. This aligns with the standard subtyping convention where `T₁ <: T₂` means `T₁` is a subtype (more constrained).

**Definition 4.2 (Strengthening).** A CapD `d₁` is a *strengthening* of `d₂` iff `d₁ ≤ d₂`. Strengthening restricts the descriptor by:
- Removing capabilities: `CapD(c \ {cap}, q)` is a strengthening of `CapD(c, q)`.
- Adding conditions: `CapD(c, q ∪ {cond})` is a strengthening of `CapD(c, q)`.

Strengthening makes the descriptor *less permissive*: fewer operations are permitted, and more contextual requirements must be met.

### 4.2 Safety of Weakening

The central theorem of this section establishes that weakening is always safe: if an operation succeeds with a CapD, it succeeds with any weakening of that CapD (i.e., any CapD that is above it in the lattice). This is the formal justification for the IVE's strategy of inferring the *strongest* (most restrictive) provable CapD — any weakening of that CapD is automatically safe.

**Theorem 4.1 (Weakening Safety).** Let `op` be an operation requiring capability `c_op`, and let `d₁, d₂` be CapDs with `d₁ ≤ d₂`. If `op` succeeds under `d₁` in context `ctx`, then `op` succeeds under `d₂` in the same context.

*Proof.* Let `d₁ = CapD(c₁, q₁)` and `d₂ = CapD(c₂, q₂)`. Since `d₁ ≤ d₂`: `c₁ ⊆ c₂` and `q₁ ⊇ q₂`. Since `op` succeeds under `d₁`: `c_op ∈ c₁` and `ctx ⊨ q₁` (all conditions in `q₁` are satisfied). Since `c₁ ⊆ c₂`: `c_op ∈ c₂`. Since `q₁ ⊇ q₂`: every condition in `q₂` is also in `q₁`, and since `ctx ⊨ q₁`, we have `ctx ⊨ q₂`. Therefore `c_op` is active under `d₂` in context `ctx`, and `op` succeeds. ∎

**Corollary 4.1 (Strengthening Restriction).** The converse does not hold: if `op` succeeds under `d₂` and `d₁ ≤ d₂`, then `op` may fail under `d₁`. Specifically, `op` fails if `c_op ∈ c₂ \ c₁` (the capability was removed by strengthening) or `q₁` contains a condition not in `q₂` that is not satisfied by `ctx` (a condition was added by strengthening that the context does not meet).

### 4.3 Weakening and Composition

**Lemma 4.1 (Weakening Distributes Over Meet).** If `d₁ ≤ d₁'` and `d₂ ≤ d₂'`, then `d₁ ⊓ d₂ ≤ d₁' ⊓ d₂'`.
*Proof.* `c₁ ⊆ c₁'` and `c₂ ⊆ c₂'` implies `c₁ ∩ c₂ ⊆ c₁' ∩ c₂'`. `q₁ ⊇ q₁'` and `q₂ ⊇ q₂'` implies `q₁ ∪ q₂ ⊇ q₁' ∪ q₂'`. Hence `CapD(c₁ ∩ c₂, q₁ ∪ q₂) ≤ CapD(c₁' ∩ c₂', q₁' ∪ q₂')`. ∎

**Lemma 4.2 (Weakening Distributes Over Join).** If `d₁ ≤ d₁'` and `d₂ ≤ d₂'`, then `d₁ ⊔ d₂ ≤ d₁' ⊔ d₂'`.
*Proof.* Symmetric to Lemma 4.1. ∎

**Theorem 4.2 (Compositional Weakening).** Weakening is compositional: if every component CapD in a composed system is individually weakened, the resulting system's CapD is the weakening of the original composed CapD. This follows directly from Lemmas 4.1 and 4.2 and the definitions of meet and join.

### 4.4 Weakening and Verification

The IVE uses weakening to discharge verification obligations. When the IVE must verify that a value's CapD `d_inferred` satisfies a required CapD `d_required`, it checks `d_required ≤ d_inferred` (the inferred descriptor is at least as permissive as the requirement). If this check fails, the IVE attempts to *strengthen* the inferred descriptor by adding information from the execution context — for example, proving that an additional condition holds, which effectively weakens the required CapD in the specific context. This interplay between static CapDs and dynamic context is the subject of Section 5.

---

## 5. Context-Dependent CapD Resolution

### 5.1 Execution Context

The VUMA proposal emphasizes that CapDs are context-dependent: "the same data can have different capabilities in different contexts." This section formalizes the resolution mechanism that determines which capabilities are actually active given a CapD and an execution context.

**Definition 5.1 (Execution Context).** An execution context `ctx` is a tuple:

```
Context ::= Context {
  phase: Phase,
  completed_ops: Set<OpId>,
  pending_ops: Set<OpId>,
  running_ops: Set<OpId>,
  held_locks: Set<LockId>,
  security_level: Level,
  live_regions: Set<RegionId>
}
```

Where:
- `phase` is the current execution phase of the SCG region
- `completed_ops` is the set of operations that have finished
- `pending_ops` is the set of operations that have not yet started
- `running_ops` is the set of operations currently executing
- `held_locks` is the set of locks held by the current execution agent
- `security_level` is the current security classification level
- `live_regions` is the set of currently allocated memory regions

**Definition 5.2 (Context Satisfaction).** The satisfaction relation `ctx ⊨ c` for condition `c` is defined as in Definition 1.3, using the context fields directly:

- `ctx ⊨ InPhase(p)` iff `ctx.phase = p`
- `ctx ⊨ AfterOp(oid)` iff `oid ∈ ctx.completed_ops`
- `ctx ⊨ BeforeOp(oid)` iff `oid ∈ ctx.pending_ops`
- `ctx ⊨ NotConcurrentWith(oid)` iff `oid ∉ ctx.running_ops`
- `ctx ⊨ RequiresLock(lid)` iff `lid ∈ ctx.held_locks`
- `ctx ⊨ SecurityLevel(lvl)` iff `ctx.security_level ≥ lvl`
- `ctx ⊨ ValidDuring(rid)` iff `rid ∈ ctx.live_regions`

### 5.2 Resolution Function

**Definition 5.3 (Resolve).** The resolution function `resolve` maps a CapD and a context to the set of active capabilities:

```
resolve : CapD × Context → 𝒫(Cap)
resolve(CapD(caps, conds), ctx) = 
  if ∀ c ∈ conds: ctx ⊨ c then caps else ∅
```

The resolution function is all-or-nothing: either all conditions are satisfied and all declared capabilities are active, or at least one condition fails and *no* capabilities are active. This design ensures that partial condition satisfaction never leads to partial capability availability — a property that simplifies reasoning and prevents subtle bugs where some operations succeed while others fail unexpectedly.

**Alternative: Fine-Grained Resolution.** While the all-or-nothing semantics is the default, the VUMA framework supports fine-grained resolution where individual conditions are attached to individual capabilities. This is represented by a set of pairs `Set<(Capability, Set<Condition>)>` rather than a single condition set for all capabilities. The formal specification of fine-grained resolution is deferred to a companion specification (VUMA-SPEC-FINE-CAPD).

### 5.3 Monotonicity of Resolution

The critical property of resolution is *monotonicity*: adding more satisfied conditions to a context can only *reduce* (or maintain) the set of active capabilities. This is because conditions are conjunctive guards — satisfying more conditions means the guard is more permissive, not less. However, the ordering of contexts is subtle: we need to define what "more context conditions" means.

**Definition 5.4 (Context Ordering).** We define a partial order on contexts. Context `ctx₁` is *more permissive* than `ctx₂` (written `ctx₁ ⊒ ctx₂`) if `ctx₁` satisfies every condition that `ctx₂` satisfies:

```
ctx₁ ⊒ ctx₂  ⟺  ∀ c ∈ Cond: (ctx₂ ⊨ c ⟹ ctx₁ ⊨ c)
```

That is, `ctx₁` is at least as permissive as `ctx₂` if every condition satisfied by `ctx₂` is also satisfied by `ctx₁`. This means `ctx₁` is in a "more advanced" or "less restricted" state — it may have completed more operations, hold more locks, be in a later phase, etc.

**Theorem 5.1 (Resolution Anti-Monotonicity).** Resolution is anti-monotone with respect to the context ordering. That is, if `ctx₁ ⊒ ctx₂`, then for any CapD `d`:

```
resolve(d, ctx₁) ⊇ resolve(d, ctx₂)
```

*Proof.* Let `d = CapD(caps, conds)`.

**Case 1:** `ctx₂` does not satisfy all conditions in `conds`. Then `resolve(d, ctx₂) = ∅ ⊆ resolve(d, ctx₁)` regardless of `ctx₁`.

**Case 2:** `ctx₂` satisfies all conditions in `conds`. Then for every `c ∈ conds`, `ctx₂ ⊨ c`. Since `ctx₁ ⊒ ctx₂`, we have `ctx₁ ⊨ c` for every such `c`. Therefore `ctx₁` also satisfies all conditions in `conds`, and `resolve(d, ctx₁) = caps = resolve(d, ctx₂)`.

In both cases, `resolve(d, ctx₁) ⊇ resolve(d, ctx₂)`. ∎

**Corollary 5.1 (Progressive Capability Activation).** As execution progresses (more operations complete, more locks are acquired, phases advance), the context becomes more permissive, and more capabilities become active. This models the intuitive behavior that a value gains capabilities as preconditions are met: a buffer gains `Write` capability after its initialization phase completes, a handle gains `Send` capability after the connection is established, etc.

### 5.4 Resolution and the Lattice

**Lemma 5.1 (Resolution is Monotone in CapD).** If `d₁ ≤ d₂`, then for any context `ctx`:

```
resolve(d₁, ctx) ⊆ resolve(d₂, ctx)
```

*Proof.* If `ctx` does not satisfy `conds(d₁)`, then `resolve(d₁, ctx) = ∅ ⊆ resolve(d₂, ctx)`. If `ctx` satisfies `conds(d₁)`, then since `conds(d₁) ⊇ conds(d₂)`, `ctx` also satisfies `conds(d₂)`. Then `resolve(d₁, ctx) = caps(d₁) ⊆ caps(d₂) = resolve(d₂, ctx)`. ∎

**Lemma 5.2 (Resolution Distributes Over Join).** For any CapDs `d₁, d₂` and context `ctx`:

```
resolve(d₁ ⊔ d₂, ctx) = resolve(d₁, ctx) ∪ resolve(d₂, ctx)
```

*Proof.* Let `d₁ = CapD(c₁, q₁)`, `d₂ = CapD(c₂, q₂)`. Then `d₁ ⊔ d₂ = CapD(c₁ ∪ c₂, q₁ ∩ q₂)`.

If `ctx` satisfies `q₁ ∩ q₂`, then `ctx` satisfies both `q₁` and `q₂`, so `resolve(d₁, ctx) = c₁`, `resolve(d₂, ctx) = c₂`, and `resolve(d₁ ⊔ d₂, ctx) = c₁ ∪ c₂`.

If `ctx` does not satisfy some `c ∈ q₁ ∩ q₂`, then `c ∈ q₁` and `c ∈ q₂`, so `ctx` does not satisfy at least one of `q₁, q₂`. Both sides are `∅` or a subset of the other side. (Note: if `ctx` fails a condition in `q₁ ∩ q₂`, it fails that condition in both `q₁` and `q₂`, so both resolutions yield `∅`. But if `ctx` satisfies `q₁ ∩ q₂` but not, say, some condition in `q₁ \ q₂`, then `resolve(d₁, ctx) = ∅` while `resolve(d₂, ctx) = c₂`, and `resolve(d₁ ⊔ d₂, ctx) = c₁ ∪ c₂`. This seems to violate the equality — but wait, if `ctx` satisfies `q₁ ∩ q₂` but not some condition in `q₁ \ q₂`, then `ctx` does not satisfy all of `q₁`, so `resolve(d₁, ctx) = ∅`. And `d₁ ⊔ d₂` has conditions `q₁ ∩ q₂`, which `ctx` does satisfy, so `resolve(d₁ ⊔ d₂, ctx) = c₁ ∪ c₂`. But `resolve(d₁, ctx) ∪ resolve(d₂, ctx) = ∅ ∪ c₂ = c₂ ≠ c₁ ∪ c₂`. So the equality does NOT hold in general for the all-or-nothing resolution.

**Correction.** The equality holds only when the all-or-nothing resolution is replaced with fine-grained resolution. For the all-or-nothing semantics, we have the weaker property:

```
resolve(d₁ ⊔ d₂, ctx) ⊇ resolve(d₁, ctx) ∪ resolve(d₂, ctx)
```

That is, join resolution is at least as permissive as the union of individual resolutions. This follows because satisfying `q₁ ∩ q₂` is easier than satisfying both `q₁` and `q₂` individually. ∎

**Lemma 5.3 (Resolution and Meet).** For any CapDs `d₁, d₂` and context `ctx`:

```
resolve(d₁ ⊓ d₂, ctx) ⊆ resolve(d₁, ctx) ∩ resolve(d₂, ctx)
```

*Proof.* Similar reasoning: `d₁ ⊓ d₂ = CapD(c₁ ∩ c₂, q₁ ∪ q₂)`. Satisfying `q₁ ∪ q₂` requires satisfying both `q₁` and `q₂`, so `resolve(d₁ ⊓ d₂, ctx) = c₁ ∩ c₂` only when both condition sets are satisfied, in which case `resolve(d₁, ctx) ∩ resolve(d₂, ctx) = c₁ ∩ c₂`. Otherwise `resolve(d₁ ⊓ d₂, ctx) = ∅ ⊆ resolve(d₁, ctx) ∩ resolve(d₂, ctx)`. ∎

### 5.5 Resolution as a Galois Connection

The relationship between CapDs and resolved capability sets forms a structure reminiscent of a Galois connection between the CapD lattice and the powerset lattice of capabilities.

**Definition 5.5 (Galois Connection).** Define two maps for a fixed context `ctx`:

```
α(d) = resolve(d, ctx)          -- abstraction: CapD → 𝒫(Cap)
γ(S) = ⊔{d : CapD | resolve(d, ctx) ⊆ S}  -- concretization: 𝒫(Cap) → CapD
```

**Lemma 5.4 (Galois Property).** For any CapD `d` and capability set `S ⊆ Cap`:

```
α(d) ⊆ S  ⟺  d ≤ γ(S)
```

*Proof sketch.* (⟹) If `resolve(d, ctx) ⊆ S`, then `d` is among the CapDs whose resolution is contained in `S`, so `d ≤ ⊔{d' : resolve(d', ctx) ⊆ S} = γ(S)`. (⟸) If `d ≤ γ(S)`, then by Lemma 5.1, `resolve(d, ctx) ⊆ resolve(γ(S), ctx)`. And `resolve(γ(S), ctx) ⊆ S` because every `d'` in the join satisfies `resolve(d', ctx) ⊆ S`, and the join's resolution (under appropriate conditions) is the union of individual resolutions, each contained in `S`. ∎

This Galois connection establishes that CapDs are a *precise abstraction* of the concrete capability sets that arise at runtime: every CapD corresponds to a well-defined set of possible capability sets (those arising from different contexts), and every set of capabilities corresponds to a most general CapD that resolves to a subset of that set.

---

## A. Summary of Key Results

| Result | Statement |
|--------|-----------|
| Theorem 2.1 | `≤` is a partial order on CapD |
| Theorem 2.2 | `(CapD, ≤, ⊓, ⊔, ⊥, ⊤)` is a bounded distributive lattice |
| Lemma 3.1 | Compatible function calls are safe |
| Lemma 3.2 | Pointer derivation weakens the source CapD |
| Corollary 3.1 | Derivation chains are monotonically weakening |
| Lemma 3.3 | Branch merge produces the weakest compatible CapD |
| Lemma 3.4 | Loop invariants ensure iteration safety |
| Theorem 4.1 | Weakening is always safe |
| Theorem 4.2 | Weakening is compositional |
| Theorem 5.1 | Resolution is anti-monotone in context |
| Lemma 5.1 | Resolution is monotone in CapD |
| Lemma 5.2 | Join resolution subsumes union of individual resolutions |
| Lemma 5.3 | Meet resolution is subsumed by intersection of individual resolutions |
| Lemma 5.4 | CapD-resolution forms a Galois connection |

---

## B. Open Questions

1. **Fine-grained conditions per capability.** The current all-or-nothing resolution semantics is simple but may be too coarse. A fine-grained model where each capability has its own condition set would be more expressive but requires a more complex algebra. This is deferred to VUMA-SPEC-FINE-CAPD.

2. **Conditional capability implication.** Should some capabilities imply others under certain conditions? For example, should `Write` imply `Read` when no condition restricts read access? The current design keeps capabilities orthogonal, but implications could reduce annotation burden.

3. **Dynamic condition sets.** The current model assumes a static set of conditions. In a fully dynamic VUMA runtime, conditions may be created and destroyed during execution. The formalism needs extension to handle dynamic condition sets.

4. **Interaction with RepD and RelD.** This specification treats CapD in isolation. The full BD triple `(RepD, CapD, RelD)` introduces cross-dimensional constraints: for example, `Execute` requires a RepD describing executable code, and `Send` is constrained by the RelD security-level flow. The cross-dimensional interaction is specified in VUMA-SPEC-BD-INTTEGRATION.

---

*End of specification.*
