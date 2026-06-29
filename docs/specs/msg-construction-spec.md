# Memory State Graph (MSG) Construction Algorithm: Formal Specification

**Document ID:** VUMA-SPEC-MSG-001  
**Author:** Parham Khairkhah

**Parent Specification:** VUMA Layer 6 — Verified-Unsafe Memory Access (Proposal §3.6)

---

## 0. Preliminary Definitions

Before specifying the construction rules, we define the core domains and structures that compose the Memory State Graph.

### 0.1 Core Domains

```
NodeId      ∈ ℕ                          — SCG node identifiers
RegionId    ∈ ℕ                          — MSG region identifiers
DerivId     ∈ ℕ                          — Derivation chain identifiers
AccessId    ∈ ℕ                          — Access entry identifiers
Addr        ∈ ℤ                          — Abstract address values
Offset      ∈ ℤ                          — Byte offsets within regions
Size        ∈ ℕ⁺                         — Sizes in bytes
RepD        = Layout × Alignment × Fields — Representation Descriptors
CapD        = 𝒫(Operation)              — Capability Descriptors (sets of ops)
RelD        = 𝒫(Relation)               — Relational Descriptors
```

### 0.2 MSG Structure

A **Memory State Graph** is a tuple:

```
MSG = (R, D, A, φ)
```

where:
- **R ⊆ Region** is the set of memory regions. Each `r ∈ R` is a record:
  ```
  r = (rid : RegionId, base : Addr, size : Size,
       status : {Allocated, Freed, Stack, Mapped, Device},
       owner : NodeId, repd : RepD, capd : CapD,
       birth : ProgramPoint, death : ProgramPoint ∪ {⊥})
  ```
- **D ⊆ Derivation** is the set of pointer derivations. Each `d ∈ D` is a record:
  ```
  d = (did : DerivId, source : DerivId ∪ RegionId,
       kind : {Offset, Cast, Assign, FieldAccess},
       offset : Offset, targetRepD : RepD,
       programPoint : ProgramPoint)
  ```
- **A ⊆ Access** is the set of memory accesses. Each `a ∈ A` is a record:
  ```
  a = (aid : AccessId, deriv : DerivId, region : RegionId,
       mode : {Read, Write, Execute}, size : Size,
       programPoint : ProgramPoint, path : Path)
  ```
- **φ : Access → {✓, ✗, ?}** is the verification status function, mapping each access to proven-safe (✓), proven-unsafe (✗), or unverified (?).

### 0.3 SCG Node Types

The Semantic Computation Graph defines the following node types relevant to the MSG:

```
N_Alloc     — AllocationNode: creates a new region
N_Dealloc   — DeallocationNode: frees a region
N_Access    — AccessNode: reads from or writes to memory
N_Cast      — CastNode: reinterprets a pointer under a different RepD
N_Arith     — ArithmeticNode: computes an offset from a base pointer
N_Call      — FunctionCall: invokes a procedure
N_Control   — ControlFlow: branches or merges execution paths
```

---

## 1. MSG Construction Rules

The MSG is constructed by a forward traversal of the SCG. Each SCG node type triggers a specific transformation on the MSG. We define each transformation as a formal rule relating the MSG before processing the node (MSG_pre) to the MSG after processing (MSG_post).

### 1.1 AllocationNode → Creates New Region

When the SCG traversal encounters an `N_Alloc` node, a new region is created in the MSG and added to the region set R.

**Rule ALLOC:**

```
  n ∈ N_Alloc    n.allocSize = s    n.allocRepD = ρ    n.allocCapD = κ
  n.programPoint = pp    rid = freshRegionId()    a = freshAddr()
  ────────────────────────────────────────────────────────────────────────
  MSG_post = (R ∪ {r}, D, A, φ)

  where r = (rid, a, s, Allocated, n.id, ρ, κ, pp, ⊥)
```

**Explanation:** The allocation node `n` specifies a size `s`, a representation descriptor `ρ`, and a capability descriptor `κ`. The rule creates a fresh region `r` with a unique identifier `rid`, a fresh base address `a`, and status `Allocated`. The region's `birth` program point is set to `pp` (the node's position in the SCG), and `death` is set to `⊥` (undefined — the region is not yet freed). The owner is set to the node's identifier, linking the region back to its allocation point in the SCG. This satisfies the **origin invariant** (§3.6.2, Invariant 4): every region can be traced back to the allocation node that created it. The fresh address `a` is chosen from a monotonic counter to ensure uniqueness; in the abstract model, we require only that `a` does not overlap with any existing region in R, formally: `∀r' ∈ R_pre : [a, a+s) ∩ [r'.base, r'.base + r'.size) = ∅`.

### 1.2 DeallocationNode → Sets Region Status to Freed

When the SCG traversal encounters an `N_Dealloc` node, the target region's status transitions from `Allocated` to `Freed`.

**Rule DEALLOC:**

```
  n ∈ N_Dealloc    n.targetDeriv = did    resolveRegion(did, D, R) = r
  r.status = Allocated    n.programPoint = pp
  ────────────────────────────────────────────────────────────────────────
  MSG_post = ((R \ {r}) ∪ {r'}, D, A, φ')

  where r' = r[status ↦ Freed, death ↦ pp]
        φ'(a) = if a.region = r.rid ∧ a.programPoint > pp then ✗ else φ(a)
```

**Explanation:** The deallocation node `n` references a derivation `did` that resolves to region `r`. The rule transitions `r.status` from `Allocated` to `Freed` and records the deallocation point as `r.death = pp`. Critically, the verification function `φ` is updated: any access `a ∈ A` whose target region is `r` and whose program point is *after* `pp` is marked as `✗` (proven-unsafe) — this is a use-after-free violation. This directly enforces the **liveness invariant** (§3.6.2, Invariant 1). If the region is already `Freed`, this constitutes a double-free, and the rule signals an error. The `resolveRegion` function follows the derivation chain from `did` back to its source region: `resolveRegion(did, D, R) = r` where `r.rid = rootRegion(did, D)`.

### 1.3 AccessNode → Creates Access Entry Linked to Derivation

When the SCG traversal encounters an `N_Access` node, a new access entry is created in the MSG, linked to the derivation that produced the pointer being dereferenced.

**Rule ACCESS:**

```
  n ∈ N_Access    n.derivId = did    n.mode = m ∈ {Read, Write, Execute}
  n.accessSize = s    n.programPoint = pp    n.path = π
  resolveRegion(did, D, R) = r    offset(did, D) = o
  ────────────────────────────────────────────────────────────────────────
  MSG_post = (R, D, A ∪ {a}, φ ∪ {(aid, v)})

  where a = (aid, did, r.rid, m, s, pp, π)
        v = verify(a, R, D, A) ∈ {✓, ✗, ?}
```

**Explanation:** The access node `n` specifies which derivation (`did`) produces the pointer being dereferenced, the access mode `m`, the size `s` of the access, and the program point `pp`. The rule creates an access entry `a` linking the derivation to the target region. The verification function `verify(a, R, D, A)` checks four conditions corresponding to the VUMA invariants:

1. **Liveness** (Invariant 1): `r.status = Allocated ∧ pp ∈ [r.birth, r.death)` — the region is alive at the access point.
2. **Bounds** (subset of Liveness): `0 ≤ o ∧ o + s ≤ r.size` — the accessed range falls within the region.
3. **Exclusivity** (Invariant 2): For `m = Write`, `∄a' ∈ A : a'.region = r.rid ∧ a'.mode = Write ∧ a'.path ∥ a.path ∧ rangesOverlap(a, a')` — no conflicting concurrent write access exists.
4. **Interpretation** (Invariant 3): The bytes `[o, o+s)` within region `r` are interpretable according to the derivation's `targetRepD` — the representation descriptor at the access offset is compatible with `n`'s expected interpretation.

If all four checks pass, `v = ✓`. If any check definitively fails, `v = ✗`. If a check cannot be resolved (e.g., due to path sensitivity or abstraction), `v = ?`. The access is recorded regardless of verification status — the IVE may refine `v` later as more information becomes available.

### 1.4 CastNode → Creates New Derivation with Cast RepD

When the SCG traversal encounters an `N_Cast` node, a new derivation is created that records the reinterpretation of a pointer under a different representation descriptor.

**Rule CAST:**

```
  n ∈ N_Cast    n.sourceDeriv = did_s    n.targetRepD = ρ_t
  n.programPoint = pp    d_new_id = freshDerivId()
  d_s ∈ D    d_s.did = did_s    resolveRegion(did_s, D, R) = r
  offset(did_s, D) = o    ρ_t.alignment = al    o mod al = 0
  ────────────────────────────────────────────────────────────────────────
  MSG_post = (R, D ∪ {d_new}, A, φ)

  where d_new = (d_new_id, did_s, Cast, o, ρ_t, pp)
```

**Explanation:** The cast node `n` takes a source derivation `did_s` and produces a new derivation `d_new` with kind `Cast`. The offset from the source derivation is preserved (casts do not change the address, only the interpretation). The `targetRepD` of the new derivation is `ρ_t`, specified by the cast node. The side condition `o mod al = 0` ensures alignment: the target representation descriptor requires alignment `al`, and the offset `o` must satisfy this alignment constraint. If the alignment check fails, the cast creates an **invalid derivation** — any subsequent access through `d_new` will fail the interpretation invariant check. This rule is critical for the **interpretation invariant** (§3.6.2, Invariant 3): the IVE must verify that the bytes at offset `o` within region `r` are valid under `ρ_t`. The derivation chain is extended: `d_new.source = did_s`, maintaining the full provenance from allocation through all intermediate derivations to the final access point.

### 1.5 ArithmeticNode → Creates Offset Derivation from Source

When the SCG traversal encounters an `N_Arith` node, a new derivation is created that records the computation of an offset from a base pointer.

**Rule ARITH:**

```
  n ∈ N_Arith    n.sourceDeriv = did_s    n.computedOffset = Δ
  n.programPoint = pp    d_new_id = freshDerivId()
  d_s ∈ D    d_s.did = did_s    resolveRegion(did_s, D, R) = r
  offset(did_s, D) = o_s    o_new = o_s + Δ
  0 ≤ o_new    o_new ≤ r.size
  ────────────────────────────────────────────────────────────────────────
  MSG_post = (R, D ∪ {d_new}, A, φ)

  where d_new = (d_new_id, did_s, Offset, o_new, d_s.targetRepD, pp)
```

**Explanation:** The arithmetic node `n` computes an offset `Δ` from the source derivation's current position. The new derivation `d_new` has kind `Offset` and records the absolute offset `o_new = o_s + Δ` within the source region `r`. The side conditions `0 ≤ o_new` and `o_new ≤ r.size` enforce the **bounds invariant**: the derived pointer must remain within the allocated region. If these conditions cannot be statically verified (e.g., `Δ` depends on runtime input), the derivation is marked with a bounds verification debt, and any subsequent access through this derivation will have `v = ?` until the bounds can be proven. The `targetRepD` of the new derivation defaults to the source derivation's RepD — the arithmetic operation changes the address but not the interpretation. A subsequent `CastNode` can change the RepD if needed. This two-step decomposition (offset then cast) mirrors the physical reality of pointer arithmetic in C-like systems, where computing an offset and reinterpreting the result are separate operations. The derivation chain preserves the full computation: from `r.base + o_s` to `r.base + o_new`, each step recorded with its program point, enabling the IVE to trace any pointer back to its allocation.

### 1.6 FunctionCall → Inlines Callee's MSG or Creates Boundary

When the SCG traversal encounters an `N_Call` node, the callee's MSG is either inlined into the caller's MSG (if the callee's MSG is available and small enough) or a function boundary is created (if the callee is external, too large, or not yet analyzed).

**Rule CALL-INLINE:**

```
  n ∈ N_Call    n.callee = f    MSG_f = (R_f, D_f, A_f, φ_f) available
  n.arguments = [(did_1, ρ_1), ..., (did_k, ρ_k)]
  n.programPoint = pp    R_f' = rename(R_f, pp)
  D_f' = rename(D_f, pp, did_1↦f_param_1, ..., did_k↦f_param_k)
  A_f' = rename(A_f, pp, path_prefix = n.callPath)
  ────────────────────────────────────────────────────────────────────────
  MSG_post = (R ∪ R_f', D ∪ D_f', A ∪ A_f', φ ∪ φ_f')
```

**Rule CALL-BOUNDARY:**

```
  n ∈ N_Call    n.callee = f    MSG_f not available or too large
  n.arguments = [(did_1, ρ_1), ..., (did_k, ρ_k)]
  n.programPoint = pp
  ────────────────────────────────────────────────────────────────────────
  MSG_post = (R, D ∪ {d_boundary}, A, φ ∪ {(aid_boundary, ?)})

  where d_boundary = (freshDerivId(), ⊥, Boundary, 0, ρ_unknown, pp)
        ∀did_i : createBoundaryAccess(did_i, pp, n.callPath)
```

**Explanation:** The call-inline rule composes the caller's MSG with the callee's MSG. The `rename` function applies a namespace prefix (based on the call site's program point `pp`) to all identifiers in the callee's MSG, preventing collisions. Argument derivations from the caller (`did_1, ..., did_k`) are mapped to the callee's formal parameter derivations (`f_param_1, ..., f_param_k`), establishing the derivation chain across the call boundary. Access paths within the callee are prefixed with the call path, enabling path-sensitive analysis. The call-boundary rule handles cases where the callee's MSG cannot be inlined: an opaque derivation `d_boundary` is created with kind `Boundary` and unknown RepD. All pointer arguments are recorded as boundary accesses with verification status `?` (unverified). This conservative treatment ensures soundness: the IVE will flag any access through a boundary derivation as unverified, prompting either inline analysis of the callee or manual verification. The boundary mechanism is the MSG's interface to compositional reasoning: by summarizing a function's memory effects as a contract (precondition/postcondition on regions and derivations), the IVE can verify callers without inlining, at the cost of some precision.

### 1.7 ControlFlow → Splits MSG into Paths, Merges with Join Rules

When the SCG traversal encounters an `N_Control` node, the MSG is split into separate execution paths (at branch points) and merged (at join points).

**Rule BRANCH:**

```
  n ∈ N_Control    n.kind = Branch    n.condition = c
  n.trueSuccessor = n_t    n.falseSuccessor = n_f
  MSG_pre = (R, D, A, φ)    π = currentPath
  ────────────────────────────────────────────────────────────────────────
  MSG_post = { (π·⊤, MSG_t), (π·⊥, MSG_f) }

  where MSG_t = (R, D, A, φ)  — to be extended by n_t's successors
        MSG_f = (R, D, A, φ)  — to be extended by n_f's successors
```

**Rule JOIN:**

```
  n ∈ N_Control    n.kind = Join
  n.predecessors = {n_1, ..., n_k}
  MSG_i = (R_i, D_i, A_i, φ_i) for each i ∈ {1, ..., k}
  ────────────────────────────────────────────────────────────────────────
  MSG_post = (⋂_i R_i^live, ⋃_i D_i, ⋃_i A_i, ⋃_i φ_i)

  where R_i^live = { r ∈ R_i | r.status = Allocated }
```

**Explanation:** The branch rule creates two copies of the current MSG, one for each successor path. The path `π` is extended with `⊤` (true branch) or `⊥` (false branch), creating distinct path identifiers for the two executions. Each copy is then independently extended by the successors. The join rule merges the MSGs from all incoming paths. The region set is intersected over *live* regions: only regions that are allocated on *all* incoming paths survive the join. A region that is freed on one path but live on another results in a region with status that depends on the path — this is handled by the path-sensitive MSG (Section 3). The derivation and access sets are unioned, as they are path-indexed and do not conflict. The verification function is also unioned; if an access is proven safe on one path but unverified on another, the merged status is the weaker of the two (ordering: `✓ > ? > ✗`). This ensures that the join rule is sound: a merged MSG never proves more than what all individual path MSGs can prove. The join rule is the key mechanism for handling loops (where the join point is the loop header) and conditionals (where the join point is the post-dominator).

---

## 2. Derivation Chain Construction

A derivation chain records the complete provenance of a pointer from its allocation point to every point of use. This section formalizes the construction of derivation chains as inference rules.

### 2.1 Definition: Derivation Chain

A **derivation chain** for derivation `d_k` is a sequence of derivations:

```
chain(d_k) = [d_0, d_1, ..., d_k]
```

where `d_0` is the root derivation (whose source is a region), and for each `i > 0`, `d_i.source = d_{i-1}.did`. The chain captures every transformation applied to the pointer: offsets, casts, assignments, and field accesses.

The **offset function** computes the cumulative offset from the region base:

```
offset(d_0, D) = 0
offset(d_i, D) = offset(d_{i-1}, D) + d_i.offset   (if d_i.kind = Offset)
offset(d_i, D) = offset(d_{i-1}, D)                  (otherwise)
```

### 2.2 Inference Rules for Derivation Chain Construction

**Rule CHAIN-ROOT** — Starting from an AllocationNode, the root derivation is created:

```
  n ∈ N_Alloc    r = new region from Rule ALLOC
  ────────────────────────────────────────
  chain(d_root) = [d_root]

  where d_root = (freshDerivId(), r.rid, Assign, 0, r.repd, n.programPoint)
```

This rule states that every allocation produces a root derivation `d_root` whose source is the region itself (not another derivation), with offset 0 and RepD equal to the region's allocation RepD.

**Rule CHAIN-ASSIGN** — Each pointer assignment creates a Derivation:

```
  chain(d_s) = [d_0, ..., d_s]    n assigns d_s to new variable
  d_new = (freshDerivId(), d_s.did, Assign, offset(d_s, D), d_s.targetRepD, pp)
  ────────────────────────────────────────
  chain(d_new) = [d_0, ..., d_s, d_new]
```

This rule extends the derivation chain with a new `Assign` derivation. An assignment does not change the offset or RepD — it creates an alias. Both `d_s` and `d_new` refer to the same memory location, but through different derivation chains. This is crucial for alias analysis: the IVE can determine that two derivations are aliases by checking whether they resolve to the same (region, offset) pair.

**Rule CHAIN-OFFSET** — Each arithmetic operation extends the chain:

```
  chain(d_s) = [d_0, ..., d_s]    n ∈ N_Arith    n.computedOffset = Δ
  o_new = offset(d_s, D) + Δ
  d_new = (freshDerivId(), d_s.did, Offset, o_new, d_s.targetRepD, pp)
  ────────────────────────────────────────
  chain(d_new) = [d_0, ..., d_s, d_new]
```

This rule records the offset computation as a new link in the derivation chain. The offset is accumulated: `d_new.offset = o_new = d_s.offset + Δ`. The RepD is preserved; a subsequent cast can change it. The IVE can verify the bounds invariant by checking `0 ≤ o_new ≤ r.size` where `r = resolveRegion(d_s, D, R)`.

**Rule CHAIN-CAST** — Each cast creates a new branch:

```
  chain(d_s) = [d_0, ..., d_s]    n ∈ N_Cast    n.targetRepD = ρ_t
  o = offset(d_s, D)    o mod ρ_t.alignment = 0
  d_new = (freshDerivId(), d_s.did, Cast, o, ρ_t, pp)
  ────────────────────────────────────────
  chain(d_new) = [d_0, ..., d_s, d_new]
```

A cast creates a *branch* in the derivation chain: from the same source derivation `d_s`, multiple derivations with different RepDs may exist. For example, the same memory may be cast to both `uint32[]` and `float32[]` at the same offset — each cast creates a separate derivation chain branch. The alignment check is a side condition: if the target RepD requires 8-byte alignment and the offset is 4, the cast is invalid and the derivation is flagged.

**Rule CHAIN-FIELD** — Field access creates a specialized offset derivation:

```
  chain(d_s) = [d_0, ..., d_s]    n accesses field f of d_s
  ρ_s = d_s.targetRepD    ρ_s.fields[f] = (f_offset, f_size, f_repd)
  d_new = (freshDerivId(), d_s.did, FieldAccess, offset(d_s, D) + f_offset, f_repd, pp)
  ────────────────────────────────────────
  chain(d_new) = [d_0, ..., d_s, d_new]
```

Field access is a special case of offset derivation where the offset and target RepD are determined by the field descriptor within the source RepD. This provides stronger verification: the IVE can check not only bounds and alignment but also that the field offset is valid within the source RepD's layout.

### 2.3 Properties of Derivation Chains

**Theorem (Origin Invariant).** For every derivation `d ∈ D`, `resolveRegion(d, D, R)` returns a region `r ∈ R` such that `r.owner` is the `N_Alloc` node that created the region.

*Proof sketch.* By induction on the length of the derivation chain. Base case: `d_root.source = r.rid` and `r.owner = N_Alloc` by construction (Rule CHAIN-ROOT). Inductive step: if `d_i.source = d_{i-1}.did`, then `resolveRegion(d_i, D, R) = resolveRegion(d_{i-1}, D, R) = r` by the induction hypothesis. ∎

**Theorem (Chain Finiteness).** For any derivation `d`, `|chain(d)|` is finite and bounded by the number of SCG nodes on the path from the allocation to the use.

*Proof sketch.* Each derivation in the chain corresponds to a distinct SCG node (by the `programPoint` field). The SCG is a finite DAG, so the number of nodes on any path is finite. ∎

**Theorem (Derivation Uniqueness).** If `chain(d_1) = chain(d_2)`, then `offset(d_1, D) = offset(d_2, D)` and `resolveRegion(d_1, D, R) = resolveRegion(d_2, D, R)`.

*Proof.* Directly from the definition of `chain` and `offset`. Equal chains imply equal sequences of offsets and casts, producing equal cumulative offsets and the same root region. ∎

---

## 3. Path-Sensitive MSG

Different execution paths through the SCG may produce different memory states. A single monolithic MSG cannot capture this path-dependence without losing precision. We therefore define the MSG as a set of path-indexed memory states.

### 3.1 Formal Definition

A **Path-Sensitive MSG** is a finite set of pairs:

```
PS-MSG = { (π₁, M₁), (π₂, M₂), ..., (πₙ, Mₙ) }
```

where:
- **Path** `πᵢ ∈ BranchDecision*` is a sequence of branch decisions. Each `BranchDecision` is a pair `(nodeId, outcome)` where `outcome ∈ {⊤, ⊥}`.
- **MemoryState** `Mᵢ = (Rᵢ, Dᵢ, Aᵢ, φᵢ)` is a tuple of regions, derivations, accesses, and verification statuses, as defined in §0.2.

We define the following operations on paths:

```
π₁ ≼ π₂  ≡  π₁ is a prefix of π₂       (path π₁ is an ancestor of π₂)
π₁ ⊥ π₂   ≡  ¬(π₁ ≼ π₂) ∧ ¬(π₂ ≼ π₁)  (paths diverge at some branch)
```

### 3.2 Path-Sensitive Construction Rules

**Rule PS-BRANCH:**

```
  (π, M) ∈ PS-MSG    n ∈ N_Control    n.kind = Branch
  ────────────────────────────────────────────────────────────────────────
  PS-MSG' = (PS-MSG \ {(π, M)}) ∪ { (π·(n.id, ⊤), M), (π·(n.id, ⊥), M) }
```

At a branch point, the current path `π` is split into two paths, each extended with the branch decision. The memory state `M` is duplicated for both paths — subsequent nodes will modify each copy independently.

**Rule PS-MERGE:**

```
  (π₁, M₁) ∈ PS-MSG    (π₂, M₂) ∈ PS-MSG
  π₁ ⊥ π₂    π₁ and π₂ converge at join node n_j
  M₁ = (R₁, D₁, A₁, φ₁)    M₂ = (R₂, D₂, A₂, φ₂)
  ────────────────────────────────────────────────────────────────────────
  PS-MSG' = (PS-MSG \ {(π₁, M₁), (π₂, M₂)}) ∪ { (π₁⊙π₂, M_join) }

  where M_join = (R_join, D₁ ∪ D₂, A₁ ∪ A₂, φ_join)
        R_join = R₁ ∩_live R₂    (see below)
        φ_join(a) = φ₁(a) ⊓ φ₂(a)  (glb in the lattice ✓ > ? > ✗)
        π₁⊙π₂ = the merge of paths π₁ and π₂ at n_j
```

The intersection `R₁ ∩_live R₂` is defined as:

```
R₁ ∩_live R₂ = { r | r.rid ∈ (liveRegionIds(R₁) ∩ liveRegionIds(R₂)) }
               ∪ { r_f | r_f.rid ∈ (freedOnOnePath(R₁, R₂)) }

where:
  liveRegionIds(R) = { r.rid | r ∈ R ∧ r.status = Allocated }
  freedOnOnePath(R₁, R₂) = { r.rid | (r₁ ∈ R₁ ∧ r₁.status = Freed) ⊻ (r₂ ∈ R₂ ∧ r₂.status = Freed) }
```

For regions that are freed on one path but live on another, the merged region retains both possibilities: the merged region's status becomes `PathDependent(freedOn={π₁}, liveOn={π₂})`. This status is resolved at each access: if the access is on a path where the region is freed, it is flagged as a use-after-free; if on a path where the region is live, it proceeds normally.

### 3.3 Access Verification Under Path Sensitivity

When verifying an access `a` under a path-sensitive MSG, the IVE considers only those memory states whose paths are compatible with the access's path:

```
verify_path_sensitive(a, PS-MSG) = v

  where S = { M | (π, M) ∈ PS-MSG ∧ π ≼ a.path }
        V = { verify(a, M) | M ∈ S }
        v = ⨓ V    (greatest lower bound of all verification results)
```

If the access is proven safe on *all* compatible paths, `v = ✓`. If it is proven unsafe on *any* compatible path, `v = ✗`. If it is unverified on some paths and safe on others, `v = ?`. This conservative merging ensures soundness: a path-sensitive MSG never proves an access safe that would be unsafe on some feasible execution path.

### 3.4 Path Pruning

To prevent path explosion (exponential growth in the number of path-state pairs), the PS-MSG employs **path pruning**:

**Definition (Path Equivalence).** Two paths `π₁, π₂` are equivalent with respect to memory state if they produce identical memory states:

```
π₁ ≡_M π₂  ≡  M(π₁) = M(π₂)
```

where `M(π)` denotes the memory state associated with path `π`.

**Rule PS-PRUNE:**

```
  (π₁, M₁) ∈ PS-MSG    (π₂, M₂) ∈ PS-MSG    M₁ = M₂
  ────────────────────────────────────────────────────────────────────────
  PS-MSG' = (PS-MSG \ {(π₁, M₁), (π₂, M₂)}) ∪ { (π₁ ∪ π₂, M₁) }
```

When two paths produce identical memory states, they are merged into a single path (represented as a set of branch decisions that lead to the same state). This is the key mechanism for preventing path explosion: after a join point, if the branch condition does not affect memory state (e.g., a branch that prints a debug message but does not modify memory), the two paths collapse into one.

### 3.5 Widening for Loop Handling

For loops, the PS-MSG applies a **widening operator** to ensure convergence:

```
  M_n = M_{n-1} ▽ (M_n^raw - M_{n-1})
```

where `▽` is the widening operator that abstracts the differences between the memory state at loop iteration `n-1` and the raw state at iteration `n`. The widening operator:
- Replaces concrete offset ranges with intervals: if offset is 0, 4, 8 after three iterations, widen to [0, ∞).
- Replaces finite region sets with region classes (see Section 4).
- Replaces concrete derivation chains with pattern summaries.

The widened state over-approximates all possible memory states reachable through the loop, ensuring that the PS-MSG converges in finite time while maintaining soundness.

---

## 4. MSG Abstraction for Scalability

The concrete MSG, as defined in Sections 1–3, can grow prohibitively large for real-world programs. This section defines an **abstract MSG** that groups similar regions and summarizes derivation chains, trading precision for scalability while preserving soundness.

### 4.1 Abstract Domains

**Abstract Region (Region Class).** A region class `rc` groups regions with similar properties:

```
rc = (rcid : RegionClassId,
      allocSite : 𝒫(NodeId),        — set of allocation nodes
      sizeRange : (Size, Size),       — (minSize, maxSize) interval
      status : {Allocated, Freed, Mixed},
      repdClass : RepDClass,          — abstract RepD
      capdClass : CapDClass,          — abstract CapD
      lifetimeRange : (PP, PP))       — (earliestBirth, latestDeath)
```

A region class `rc` **represents** a concrete region `r` (written `r ⊑ rc`) if:

```
r.owner ∈ rc.allocSite
∧ r.size ∈ [rc.sizeRange.min, rc.sizeRange.max]
∧ r.repd ⊑ rc.repdClass
∧ r.capd ⊆ rc.capdClass
∧ r.birth ≥ rc.lifetimeRange.min
∧ (rc.status = Mixed ∨ r.status = rc.status)
```

**Abstract Derivation.** An abstract derivation summarizes a family of concrete derivation chains by their pattern:

```
d̂ = (did : DerivId,
      sourceClass : RegionClassId,
      pattern : DerivPattern,
      offsetRange : (Offset, Offset),
      repdClass : RepDClass)

DerivPattern ::= Root
               | OffsetPat(DerivPattern, OffsetRange)
               | CastPat(DerivPattern, RepDClass)
               | AssignPat(DerivPattern)
               | FieldPat(DerivPattern, FieldName)
               | BoundaryPat
```

An abstract derivation `d̂` **represents** a concrete derivation `d` (written `d ⊑ d̂`) if:

```
resolveRegion(d, D, R) ⊑ rc   (where rc = region class with rcid = d̂.sourceClass)
∧ chain(d) matches d̂.pattern
∧ offset(d, D) ∈ [d̂.offsetRange.min, d̂.offsetRange.max]
∧ d.targetRepD ⊑ d̂.repdClass
```

### 4.2 Abstract MSG Definition

An **Abstract MSG** is a tuple:

```
A-MSG = (RC, D̂, Â, φ̂)
```

where:
- `RC` is a set of region classes
- `D̂` is a set of abstract derivations
- `Â` is a set of abstract accesses (grouped by region class and derivation class)
- `φ̂ : Â → {✓, ?, ✗}` is the abstract verification function

### 4.3 Abstraction Function

The **abstraction function** `α : Concrete MSG → Abstract MSG` maps a concrete MSG to its abstract representation:

```
α(MSG) = A-MSG

where:
  RC = { rc | ∃r ∈ R : r ⊑ rc }           — group regions into classes
  D̂ = { d̂ | ∃d ∈ D : d ⊑ d̂ }             — group derivations by pattern
  Â = { â | ∃a ∈ A : a ⊑ â }               — group accesses by class
  φ̂(â) = ⨓ { φ(a) | a ∈ A ∧ a ⊑ â }       — merge verification results
```

### 4.4 Concretization Function

The **concretization function** `γ : Abstract MSG → 𝒫(Concrete MSG)` maps an abstract MSG to the set of all concrete MSGs it represents:

```
γ(A-MSG) = { MSG | α(MSG) ⊑ A-MSG }
```

where `⊑` on abstract MSGs is the pointwise ordering on region classes, derivations, and verification results.

### 4.5 Galois Connection

The abstraction and concretization functions form a **Galois connection**:

```
𝒫(Concrete MSG)  ⇄  Abstract MSG
       γ                  α

satisfying:  α(MSG) ⊑ A-MSG  ⇔  MSG ∈ γ(A-MSG)
```

This ensures that the abstraction is sound: any property proved about the abstract MSG holds for all concrete MSGs it represents.

### 4.6 Soundness Theorem

**Theorem (Abstract Soundness).** If the abstract MSG proves an invariant (i.e., `φ̂(â) = ✓` for some abstract access `â`), then the invariant also holds in every concrete MSG represented by the abstract MSG.

*Formally:*

```
∀A-MSG, â ∈ Â : φ̂(â) = ✓ ⇒ ∀MSG ∈ γ(A-MSG), ∀a ∈ A : a ⊑ â ⇒ φ(a) = ✓
```

*Proof sketch.* Assume `φ̂(â) = ✓`. By definition of `φ̂`, `⨓ { φ(a) | a ∈ A ∧ a ⊑ â } = ✓`. Since `✓` is the greatest element of the verification lattice, every `φ(a)` in the meet must be `✓`. Therefore, for every concrete access `a ⊑ â`, `φ(a) = ✓`. ∎

### 4.7 Precision vs. Performance Tradeoff

The abstraction introduces imprecision in two ways:

1. **Region class merging**: Two regions with different sizes or RepDs may be grouped into the same class, causing the abstract verification to be less precise than the concrete verification. Example: regions of size 64 and 128 merged into a class with `sizeRange = (64, 128)` — an offset of 100 is valid for the larger region but invalid for the smaller one; the abstract verification must conservatively report `?`.

2. **Derivation pattern summarization**: Long derivation chains are summarized by patterns, losing information about intermediate steps. Example: a chain of 10 offset operations summarized as `OffsetPat(Root, [0, 1000))` — the abstract MSG knows the final offset is in [0, 1000) but not the exact value, potentially missing bounds violations at specific offsets.

The **precision knob** is the granularity of region classes and derivation patterns. Finer granularity (more classes, more specific patterns) yields more precise verification but higher computational cost. Coarser granularity yields faster analysis but more `?` results. The IVE can dynamically adjust the precision: start with coarse abstractions, and refine only for accesses that yield `?` — a form of **counterexample-guided abstraction refinement (CEGAR)** adapted for memory verification.

### 4.8 Widening and Narrowing

To ensure termination of the abstract construction, we define widening and narrowing operators on the abstract domains:

**Widening on region classes:**

```
rc₁ ▽ rc₂ = rc_merged

where rc_merged.allocSite = rc₁.allocSite ∪ rc₂.allocSite
      rc_merged.sizeRange = (min(rc₁.sizeRange.min, rc₂.sizeRange.min),
                             max(rc₁.sizeRange.max, rc₂.sizeRange.max))
      rc_merged.status = if rc₁.status = rc₂.status then rc₁.status else Mixed
```

**Narrowing on region classes:**

```
rc₁ △ rc₂ = rc_narrowed

where rc_narrowed.sizeRange = intersect(rc₁.sizeRange, rc₂.sizeRange)
      rc_narrowed.status = rc₂.status  (use the more precise status)
```

Widening is applied during fixpoint iteration to ensure convergence; narrowing is applied afterward to recover some of the precision lost by widening. This is the standard approach from abstract interpretation, adapted to the MSG domain.

---

## 5. Incremental MSG Update

When a node is added to the SCG (e.g., during interactive editing), reconstructing the entire MSG from scratch is wasteful. This section defines an **incremental update** mechanism that computes only the delta — the parts of the MSG that change.

### 5.1 Delta-MSG Definition

A **Delta-MSG** captures the changes to an MSG resulting from the addition of a single SCG node `n`:

```
Δ-MSG(n) = (ΔR, ΔD, ΔA, Δφ)
```

where:
- `ΔR = (R⁺, R⁻, R~)` — regions added (`R⁺`), removed (`R⁻`), and modified (`R~`)
- `ΔD = (D⁺, D⁻, D~)` — derivations added, removed, and modified
- `ΔA = (A⁺, A⁻, A~)` — accesses added, removed, and modified
- `Δφ` — changes to the verification function

The **incremental update** applies the delta to the existing MSG:

```
MSG ⊕ Δ-MSG = MSG'

where R' = (R \ R⁻) ∪ R⁺ ∪ R~
      D' = (D \ D⁻) ∪ D⁺ ∪ D~
      A' = (A \ A⁻) ∪ A⁺ ∪ A~
      φ' = φ ∪ Δφ    (Δφ overrides existing entries)
```

### 5.2 Delta Computation Rules

Each SCG node type produces a specific delta:

**Delta-ALLOC:**

```
  n ∈ N_Alloc added to SCG
  ────────────────────────────────────────
  Δ-MSG(n) = ({r}, ∅, ∅, ∅)

  where r = new region per Rule ALLOC
```

**Delta-DEALLOC:**

```
  n ∈ N_Dealloc added to SCG
  ────────────────────────────────────────
  Δ-MSG(n) = (∅, ∅, ∅, ∅, Δφ)

  where Δφ = { (a.aid, ✗) | a ∈ A ∧ a.region = r.rid ∧ a.programPoint > n.programPoint }
        r = target region of n
```

**Delta-ACCESS:**

```
  n ∈ N_Access added to SCG
  ────────────────────────────────────────
  Δ-MSG(n) = (∅, ∅, {a}, {(a.aid, v)})

  where a = new access per Rule ACCESS
        v = verify(a, R, D, A)
```

**Delta-CAST:**

```
  n ∈ N_Cast added to SCG
  ────────────────────────────────────────
  Δ-MSG(n) = (∅, {d_new}, ∅, ∅)

  where d_new = new derivation per Rule CAST
```

**Delta-ARITH:**

```
  n ∈ N_Arith added to SCG
  ────────────────────────────────────────
  Δ-MSG(n) = (∅, {d_new}, ∅, ∅)

  where d_new = new derivation per Rule ARITH
```

**Delta-CALL:**

```
  n ∈ N_Call added to SCG
  ────────────────────────────────────────────────────────────────────────
  Δ-MSG(n) = (R_f', D_f', A_f', φ_f')    (if inlining)

  Δ-MSG(n) = (∅, {d_boundary}, {boundary_accesses}, Δφ_boundary)    (if boundary)
```

**Delta-CONTROL:**

```
  n ∈ N_Control (Branch) added to SCG
  ────────────────────────────────────────
  Δ-MSG(n) = path-split of existing MSG per Rule PS-BRANCH

  n ∈ N_Control (Join) added to SCG
  ────────────────────────────────────────
  Δ-MSG(n) = path-merge of incoming MSGs per Rule PS-MERGE
```

### 5.3 Propagation Rules for Delta Updates

When a delta modifies a region, derivation, or access that is referenced by other parts of the MSG, the change must be **propagated**. Propagation follows the dependency graph of the MSG.

**Definition (Dependency Graph).** The MSG dependency graph `G_dep = (V, E)` has:
- Vertices `V = R ∪ D ∪ A` (all regions, derivations, and accesses)
- Edges `E = { (d, r) | d ∈ D, r = resolveRegion(d) } ∪ { (a, d) | a ∈ A, d = a.deriv } ∪ { (d, d') | d ∈ D, d' ∈ D, d.source = d'.did }`

**Rule PROPAGATE-REGION:**

```
  r ∈ R~    (region r is modified)
  D_r = { d ∈ D | resolveRegion(d) = r }    (all derivations into r)
  A_r = { a ∈ A | a.region = r.rid }        (all accesses into r)
  ────────────────────────────────────────────────────────────────────────
  ∀d ∈ D_r : re-verify bounds(d, r)
  ∀a ∈ A_r : re-verify φ(a)
  Δφ_propagated = { (a.aid, verify(a, R, D, A)) | a ∈ A_r }
```

If a region's size, status, or RepD changes, all derivations that target the region must be re-verified for bounds, and all accesses must be re-verified for liveness, exclusivity, and interpretation.

**Rule PROPAGATE-DERIVATION:**

```
  d ∈ D~    (derivation d is modified)
  D_downstream = { d' ∈ D | d' is transitively derived from d }
  A_downstream = { a ∈ A | a.deriv ∈ D_downstream ∪ {d} }
  ────────────────────────────────────────────────────────────────────────
  ∀d' ∈ D_downstream : recompute offset(d')
  ∀a ∈ A_downstream : re-verify φ(a)
  Δφ_propagated = { (a.aid, verify(a, R, D, A)) | a ∈ A_downstream }
```

If a derivation's offset or RepD changes, all downstream derivations must have their offsets recomputed, and all downstream accesses must be re-verified. The propagation is transitive: changing `d_0` affects `d_1` (which derives from `d_0`), which affects `d_2` (which derives from `d_1`), and so on.

**Rule PROPAGATE-ACCESS:**

```
  a ∈ A~    (access a is modified)
  ────────────────────────────────────────
  Δφ_propagated = { (a.aid, verify(a, R, D, A)) }
```

An access modification only requires re-verification of that access — it does not affect other parts of the MSG (accesses are leaves in the dependency graph).

### 5.4 Propagation Termination

**Theorem (Delta Update Finiteness).** For any single SCG node addition, the delta update and its propagation produce a finite delta, bounded by O(|D_downstream| + |A_affected|) where `D_downstream` is the set of derivations transitively dependent on the modified derivation, and `A_affected` is the set of accesses into modified regions or through modified derivations.

*Proof sketch.* The dependency graph is a DAG (derivations form chains, accesses are leaves). A modification to node `v` propagates to all nodes reachable from `v` in the dependency graph. Since the graph is finite (bounded by |R| + |D| + |A|), the propagation visits a finite number of nodes. Moreover, the propagation is monotone: each node is visited at most once (re-verification of a node does not modify its dependencies, only its verification status). Therefore, the total propagation work is O(|reachable from v|) which is bounded by |D_downstream| + |A_affected|. ∎

**Theorem (Delta Update Boundedness).** The size of the delta `|Δ-MSG(n)|` is bounded by a constant for each node type (except `N_Call` with inlining, where it is bounded by the callee's MSG size).

*Proof.* Directly from the delta computation rules: `N_Alloc` adds one region (constant), `N_Dealloc` modifies one region and re-verifies accesses (bounded by existing MSG structure), `N_Access` adds one access (constant), `N_Cast` and `N_Arith` each add one derivation (constant), `N_Control` splits or merges paths (bounded by number of active paths). The only non-constant case is `N_Call` with inlining, where the delta size equals the callee's MSG size — but this is bounded by the callee's own structure, which is fixed at call time. ∎

### 5.5 Incremental Path-Sensitive Update

For the path-sensitive MSG, incremental updates must account for the path structure:

```
Δ-PS-MSG(n) = { (π, Δ-MSG_π(n)) | π ∈ activePaths(n) }
```

where `activePaths(n)` is the set of paths that reach node `n` in the SCG. The delta for each path is computed independently, then merged at join points. This ensures that path-sensitive information is maintained incrementally without recomputing the entire PS-MSG.

**Rule INCREMENTAL-JOIN:**

```
  Δ-PS-MSG at join node n_j
  {(π₁, Δ₁), (π₂, Δ₂)} computed for incoming paths
  ────────────────────────────────────────────────────────────────────────
  Δ_join = mergeDeltas(Δ₁, Δ₂)

  where mergeDeltas adds conflicting region statuses to PathDependent
        and merges verification results with glb
```

### 5.6 Incremental Abstraction Refinement

When an incremental update produces a `?` verification result, the IVE can **refine the abstraction** locally:

```
  Δ-MSG(n) produces φ(a) = ?
  ────────────────────────────────────────
  Refine: split the region class containing a.region
          into sub-classes that distinguish the relevant property
          re-verify a with the refined abstraction
```

This is the incremental version of CEGAR: instead of refining the entire abstract MSG, only the region classes and derivation patterns relevant to the unverified access are refined. The refinement is local and bounded by the size of the affected region class.

---

## Appendix A: Notation Summary

| Symbol | Meaning |
|--------|---------|
| `R` | Set of memory regions |
| `D` | Set of pointer derivations |
| `A` | Set of memory accesses |
| `φ` | Verification status function |
| `ρ` (RepD) | Representation Descriptor |
| `κ` (CapD) | Capability Descriptor |
| `π` | Execution path (sequence of branch decisions) |
| `⊑` | Abstraction ordering (concrete ⊑ abstract) |
| `α` | Abstraction function |
| `γ` | Concretization function |
| `▽` | Widening operator |
| `△` | Narrowing operator |
| `⨓` | Greatest lower bound (meet) |
| `⊕` | Delta application operator |
| `⊥` | Undefined / bottom element |
| `⊤, ⊥` | True branch / false branch (in paths) |

## Appendix B: Invariant Summary

| Invariant | Formal Statement | MSG Mechanism |
|-----------|-----------------|---------------|
| Liveness | `∀a ∈ A : a.programPoint ∈ [r.birth, r.death)` where `r = resolveRegion(a.deriv)` | DEALLOC updates `r.death`; ACCESS checks liveness |
| Exclusivity | `∀a₁, a₂ ∈ A : a₁.mode = Write ∧ a₂.mode ∈ {Read, Write} ∧ sameRegion(a₁, a₂) ∧ concurrent(a₁, a₂) ⇒ a₁ = a₂` | ACCESS checks for conflicting concurrent accesses |
| Interpretation | `∀a ∈ A : bytesAt(a.region, a.offset, a.size) ⊢ a.repd` | CAST checks alignment; ACCESS checks RepD compatibility |
| Origin | `∀d ∈ D : ∃r ∈ R : r = resolveRegion(d)` | CHAIN-ROOT creates root derivation from region |
| Cleanup | `∀r ∈ R : r.status = Freed ∨ r.death = ⊥ ∧ r is marked intentionally leaked` | DEALLOC transitions status; final scan checks for leaks |

---

*End of Specification*
