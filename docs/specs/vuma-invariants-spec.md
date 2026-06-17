# VUMA Global Invariants — Formal Specification

**Document ID:** VUMA-SPEC-INV-001
**Version:** 1.0
**Date:** 2026-03-05
**Author:** Agent W1-05
**Status:** Final Draft

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Memory State Graph (MSG) Formal Definition](#2-memory-state-graph-msg-formal-definition)
3. [Invariant 1: Liveness](#3-invariant-1-liveness)
4. [Invariant 2: Exclusivity](#4-invariant-2-exclusivity)
5. [Invariant 3: Interpretation](#5-invariant-3-interpretation)
6. [Invariant 4: Origin](#6-invariant-4-origin)
7. [Invariant 5: Cleanup](#7-invariant-5-cleanup)
8. [Invariant Dependency Graph](#8-invariant-dependency-graph)
9. [Formal Theorems](#9-formal-theorems)
10. [References](#10-references)

---

## 1. Introduction

This document provides the formal specification for the five global invariants of the Verified-Unsafe Memory Access (VUMA) model, as introduced in Layer 6 of the AI-Native Language Design Framework (Proposal Section 3.6).

The VUMA model inverts the traditional safety paradigm: instead of restricting access to prevent misuse, it permits unrestricted access and verifies safety through global reasoning. The Inference and Verification Engine (IVE) constructs a **Memory State Graph (MSG)** and proves that the program's memory behavior satisfies all five invariants simultaneously. If the IVE cannot prove an invariant for a specific access, it flags that access with the execution path leading to the potential violation.

The five invariants are:

| # | Invariant | Concern |
|---|-----------|---------|
| 1 | **Liveness** | Every access targets allocated memory |
| 2 | **Exclusivity** | No conflicting concurrent accesses |
| 3 | **Interpretation** | Access respects the Representation Descriptor |
| 4 | **Origin** | Every address traces to a valid allocation |
| 5 | **Cleanup** | Every allocation is eventually freed or explicitly leaked |

These invariants are **not independent** — Liveness depends on Origin, and Cleanup refines Liveness temporally. The dependency structure is made explicit in Section 8.

### 1.1 Notation Conventions

| Symbol | Meaning |
|--------|---------|
| ∀ | Universal quantification |
| ∃ | Existential quantification |
| ⇒ | Implies |
| ∧ | Logical AND |
| ∨ | Logical OR |
| ¬ | Logical NOT |
| ↦ | Maps to |
| ⊑ | Refines / is a subsequence of |
| ∘ | Function composition |
| ⌣ | Byte-range overlap |
| ⟦·⟧ | Semantic interpretation |
| PP | Program point (a node in the SCG) |
| ℕ | Natural numbers |
| Offset | Integer offset in bytes |
| RepD | Representation Descriptor |
| CapD | Capability Descriptor |
| RelD | Relational Descriptor |

---

## 2. Memory State Graph (MSG) Formal Definition

The MSG is the central data structure that the IVE constructs and reasons over. It is a typed, attributed, directed hypergraph that captures the complete memory behavior of a program.

### 2.1 Definition

$$
\text{MSG} = (\mathcal{R},\ \mathcal{D},\ \mathcal{A},\ \mathcal{S})
$$

where:

| Component | Type | Description |
|-----------|------|-------------|
| $\mathcal{R}$ | Set of **Region** | All memory regions in the program |
| $\mathcal{D}$ | Set of **Derivation** | All pointer derivations |
| $\mathcal{A}$ | Set of **Access** | All memory accesses |
| $\mathcal{S}$ | Set of **SyncEdge** | All synchronization edges |

### 2.2 Region

A Region represents a contiguous range of addresses with associated metadata:

$$
\text{Region} = \{\\
\quad \text{id} : \text{RegionId},\\
\quad \text{base\_addr} : \text{Addr},\\
\quad \text{size} : \mathbb{N},\\
\quad \text{status} : \text{Allocated} \mid \text{Freed} \mid \text{Stack} \mid \text{Mapped},\\
\quad \text{alloc\_point} : \text{PP},\\
\quad \text{free\_point} : \text{PP}\ ?\\
\}
$$

**Auxiliary definitions:**

- **Address range**: $\text{range}(r) = [r.\text{base\_addr},\ r.\text{base\_addr} + r.\text{size})$
- **Byte range overlap**: $\text{overlap}(r_1, r_2) \iff \text{range}(r_1) \cap \text{range}(r_2) \neq \emptyset$
- **Contains address**: $\text{contains}(r, a) \iff a \in \text{range}(r)$
- **Status at program point**: Defined by the allocation/deallocation ordering (see Invariant 1).

### 2.3 Derivation

A Derivation represents the computation of an address from another address or allocation:

$$
\text{Derivation} = \{\\
\quad \text{id} : \text{DerivationId},\\
\quad \text{source} : \text{Region} \mid \text{Derivation},\\
\quad \text{offset} : \text{Offset},\\
\quad \text{cast} : \text{RepD}\ ?\\
\}
$$

**Kinds of derivation:**

1. **Base derivation**: $\text{source} \in \mathcal{R}$, $\text{offset} = 0$, $\text{cast} = \text{null}$. This is the address of the first byte of a freshly allocated region.
2. **Offset derivation**: $\text{source} \in \mathcal{R} \cup \mathcal{D}$, $\text{offset} \neq 0$, $\text{cast} = \text{null}$. This is pointer arithmetic: adding a byte offset to a source address.
3. **Cast derivation**: $\text{source} \in \mathcal{D}$, $\text{offset} = 0$, $\text{cast} \neq \text{null}$. This is a RepD reinterpretation of the same address.

A derivation chain forms a directed tree rooted at a Region:

$$
\text{Derivations form a forest where every tree root is a Region.}
$$

### 2.4 Access

An Access represents a single read or write operation targeting a derived address:

$$
\text{Access} = \{\\
\quad \text{id} : \text{AccessId},\\
\quad \text{target} : \text{Derivation},\\
\quad \text{kind} : \text{Read} \mid \text{Write},\\
\quad \text{size} : \mathbb{N},\\
\quad \text{program\_point} : \text{PP}\\
\}
$$

**Auxiliary definitions:**

- **Accessed byte range**: $\text{bytes}(a) = [\text{addr}(a.\text{target}),\ \text{addr}(a.\text{target}) + a.\text{size})$
- **Access kind is write**: $\text{is\_write}(a) \iff a.\text{kind} = \text{Write}$

### 2.5 SyncEdge

A SyncEdge represents a synchronization relationship between two accesses, establishing an ordering:

$$
\text{SyncEdge} = \{\\
\quad \text{access}_1 : \text{Access},\\
\quad \text{access}_2 : \text{Access},\\
\quad \text{ordering} : \text{HappensBefore} \mid \text{Atomic} \mid \text{Locked}\\
\}
$$

**Ordering semantics:**

| Ordering | Meaning |
|----------|---------|
| **HappensBefore** | $a_1$ completes before $a_2$ begins (sequential consistency, fork-join, message passing) |
| **Atomic** | $a_1$ and $a_2$ access the same atomic variable; memory ordering guarantees visibility |
| **Locked** | $a_1$ and $a_2$ are guarded by the same lock; mutual exclusion is guaranteed |

**Ordered relation (transitive closure):**

$$
\text{ordered}(a_1, a_2) \iff \exists\ n \geq 1,\ \exists\ e_1, \ldots, e_n \in \mathcal{S} :\\
\quad e_1.\text{access}_1 = a_1 \land e_n.\text{access}_2 = a_2 \land\\
\quad \forall\ i \in [1, n):\ e_i.\text{access}_2 = e_{i+1}.\text{access}_1
$$

That is, `ordered` is the reflexive-transitive closure of the SyncEdge relation, establishing a partial order over accesses.

### 2.6 Derived Functions

**Address resolution** — compute the concrete address a derivation refers to:

$$
\text{addr}(d) = \begin{cases}
r.\text{base\_addr} + d.\text{offset} & \text{if } d.\text{source} \in \mathcal{R} \text{ (a Region)} \\
\text{addr}(d.\text{source}) + d.\text{offset} & \text{if } d.\text{source} \in \mathcal{D} \text{ (a Derivation)}
\end{cases}
$$

**Region of a derivation** — find the root Region of the derivation tree:

$$
\text{region\_of}(d) = \begin{cases}
d.\text{source} & \text{if } d.\text{source} \in \mathcal{R} \\
\text{region\_of}(d.\text{source}) & \text{if } d.\text{source} \in \mathcal{D}
\end{cases}
$$

**Well-formedness**: $\text{region\_of}$ is well-defined because derivations form a forest rooted at Regions (Section 2.3), ensuring termination.

**Effective RepD of a derivation** — the most recent cast in the derivation chain:

$$
\text{repd\_of}(d) = \begin{cases}
d.\text{cast} & \text{if } d.\text{cast} \neq \text{null} \\
\text{repd\_of}(d.\text{source}) & \text{if } d.\text{source} \in \mathcal{D} \land d.\text{cast} = \text{null} \\
\text{default\_repd}(\text{region\_of}(d)) & \text{if } d.\text{source} \in \mathcal{R} \land d.\text{cast} = \text{null}
\end{cases}
$$

where $\text{default\_repd}(r)$ is the RepD specified at the allocation point of $r$.

---

## 3. Invariant 1: Liveness

> **Every access targets allocated memory.**

### 3.1 Formal Statement

$$
\boxed{\forall\ a \in \mathcal{A} :\ \text{is\_allocated}(\text{region\_of}(a.\text{target}),\ a.\text{program\_point})}
$$

**Definition of `is_allocated`:**

$$
\text{is\_allocated}(r, pp) \iff \begin{cases}
\text{true} & \text{if } r.\text{status} = \text{Stack} \land pp \in \text{lifetime}(r) \\
\text{true} & \text{if } r.\text{status} = \text{Allocated} \land r.\text{alloc\_point} \leq_{pp} pp \land (r.\text{free\_point} = \text{null} \lor pp <_{pp} r.\text{free\_point}) \\
\text{true} & \text{if } r.\text{status} = \text{Mapped} \land \text{mapping\_valid\_at}(r, pp) \\
\text{false} & \text{otherwise}
\end{cases}
$$

where $\leq_{pp}$ is the program-point ordering (the partial order defined by the SCG's control flow edges), and $\text{lifetime}(r)$ for Stack regions is the set of program points between the frame entry and frame exit.

**Additional requirement — bounds check:**

$$
\forall\ a \in \mathcal{A} :\ \text{bytes}(a) \subseteq \text{range}(\text{region\_of}(a.\text{target}))
$$

That is, the accessed byte range must be fully contained within the source region.

### 3.2 Proof Strategy Sketch

1. **Region state analysis**: For each Region $r$, compute the set of program points where $r.\text{status} \in \{\text{Allocated}, \text{Stack}, \text{Mapped}\}$ by analyzing the allocation and deallocation points. This yields a temporal interval $[r.\text{alloc\_point},\ r.\text{free\_point})$ during which the region is live.

2. **Derivation resolution**: For each Access $a$, resolve $\text{region\_of}(a.\text{target})$ by traversing the derivation chain to its root Region. This is guaranteed to terminate by the forest structure of derivations.

3. **Containment check**: Verify that $\text{bytes}(a) \subseteq \text{range}(\text{region\_of}(a.\text{target}))$ by comparing the resolved address and access size against the region bounds. For offset derivations, verify that $\text{offset} + a.\text{size} \leq \text{region\_of}(a.\text{target}).\text{size}$.

4. **Liveness check**: Verify that $a.\text{program\_point}$ falls within the live interval of $\text{region\_of}(a.\text{target})$. This requires reasoning about the control flow graph to determine reachability.

5. **Path sensitivity**: For programs with conditional deallocation, the IVE must enumerate feasible execution paths. If liveness holds on all feasible paths, the invariant is satisfied. If any feasible path violates liveness, the IVE produces that path as a counterexample.

### 3.3 Example: Satisfying Program

```
// Simple allocation, use, and free
r = allocate(bytes[64]);     // Region r: base=0x1000, size=64, alloc_point=PP1
d = derive(r, offset=32);    // Derivation d: source=r, offset=32
a = read(d, size=4);         // Access a: target=d, kind=Read, size=4, pp=PP2
free(r);                     // r.free_point = PP3

// Verification:
//   region_of(d) = r
//   bytes(a) = [0x1020, 0x1024) ⊆ [0x1000, 0x1040) = range(r) ✓
//   is_allocated(r, PP2): PP1 ≤ PP2 < PP3 ✓
// Liveness invariant: SATISFIED
```

### 3.4 Example: Violating Program

```
// Use-after-free
r = allocate(bytes[64]);     // Region r: base=0x1000, size=64, alloc_point=PP1
free(r);                     // r.free_point = PP2
d = derive(r, offset=0);     // Derivation d: source=r, offset=0
a = read(d, size=4);         // Access a: target=d, kind=Read, size=4, pp=PP3

// Verification:
//   region_of(d) = r
//   is_allocated(r, PP3): PP2 ≤ PP3, but r.free_point = PP2
//   PP3 ≥ PP2 ⇒ NOT allocated at PP3
// Liveness invariant: VIOLATED
// IVE report: "Access a at PP3 targets freed region r (freed at PP2)"
```

---

## 4. Invariant 2: Exclusivity

> **No conflicting concurrent accesses exist without synchronization.**

### 4.1 Formal Statement

$$
\boxed{\forall\ a_1, a_2 \in \mathcal{A} :\ \text{conflicts}(a_1, a_2) \Rightarrow \text{ordered}(a_1, a_2)}
$$

**Definition of `conflicts`:**

$$
\text{conflicts}(a_1, a_2) \iff \text{is\_write}(a_1) \lor \text{is\_write}(a_2)\\
\quad \land \text{region\_of}(a_1.\text{target}) = \text{region\_of}(a_2.\text{target})\\
\quad \land \text{bytes}(a_1) \⌣\ \text{bytes}(a_2)\\
\quad \land a_1 \neq a_2
$$

where $⌣$ denotes byte-range overlap:

$$
[b_1, e_1) \⌣\ [b_2, e_2) \iff b_1 < e_2 \land b_2 < e_1
$$

**Key observations:**

- Two Read accesses never conflict (reads are always safe to overlap).
- A Read and a Write conflict if they touch overlapping bytes.
- Two Write accesses conflict if they touch overlapping bytes.
- The `ordered` relation (transitive closure of SyncEdges, defined in Section 2.5) establishes that one access completes before the other begins, preventing simultaneous execution.

### 4.2 Proof Strategy Sketch

1. **Conflict pair enumeration**: For each pair of accesses $(a_1, a_2)$ where at least one is a Write, check:
   - (a) They target the same Region: $\text{region\_of}(a_1.\text{target}) = \text{region\_of}(a_2.\text{target})$
   - (b) Their byte ranges overlap: $\text{bytes}(a_1) \⌣\ \text{bytes}(a_2)$

2. **Concurrent execution analysis**: For each conflict pair, determine whether they can execute concurrently. Two accesses are concurrent iff neither $\text{ordered}(a_1, a_2)$ nor $\text{ordered}(a_2, a_1)$ holds.

3. **Synchronization graph construction**: Build the synchronization graph $G_{\text{sync}}$ from $\mathcal{S}$. Compute the transitive closure to establish the `ordered` relation.

4. **Reachability check**: For each conflict pair $(a_1, a_2)$, verify that $a_2$ is reachable from $a_1$ (or vice versa) in $G_{\text{sync}}$. If neither direction is reachable, the accesses are concurrent and conflicting — the invariant is violated.

5. **Lock-set analysis (optional strengthening)**: For Locked SyncEdges, verify that the accesses are guarded by the same lock instance. Different lock instances do not provide mutual exclusion.

6. **Atomic access classification**: For Atomic SyncEdges, verify that the overlapping accesses are to the same atomic variable with compatible memory ordering (acquire/release, sequentially consistent).

### 4.3 Example: Satisfying Program

```
// Mutex-protected write with concurrent read
r = allocate(bytes[64]);        // Region r: base=0x1000, size=64
d1 = derive(r, offset=0);       // Derivation d1 → r[0..4)
d2 = derive(r, offset=0);       // Derivation d2 → r[0..4)

// Thread 1
lock(mutex);
a1 = write(d1, size=4);         // Access a1: pp=PP1, kind=Write
unlock(mutex);

// Thread 2
lock(mutex);
a2 = read(d2, size=4);          // Access a2: pp=PP2, kind=Read
unlock(mutex);

// SyncEdges:
//   S1 = { access1=a1, access2=a2, ordering=Locked }

// Verification:
//   conflicts(a1, a2): is_write(a1)=true, same region, bytes overlap ✓
//   ordered(a1, a2): S1 establishes a1→a2 via Locked ordering ✓
// Exclusivity invariant: SATISFIED
```

### 4.4 Example: Violating Program

```
// Data race: unsynchronized write and read
r = allocate(bytes[64]);        // Region r: base=0x1000, size=64
d1 = derive(r, offset=0);
d2 = derive(r, offset=0);

// Thread 1 (no synchronization)
a1 = write(d1, size=4);         // Access a1: pp=PP1, kind=Write

// Thread 2 (no synchronization)
a2 = read(d2, size=4);          // Access a2: pp=PP2, kind=Read

// No SyncEdge between a1 and a2

// Verification:
//   conflicts(a1, a2): is_write(a1)=true, same region, bytes overlap ✓
//   ordered(a1, a2): no path in sync graph → NOT ordered ✗
// Exclusivity invariant: VIOLATED
// IVE report: "Write a1 at PP1 and Read a2 at PP2 conflict on region r[0..4) without synchronization"
```

---

## 5. Invariant 3: Interpretation

> **Every access respects the Representation Descriptor (RepD) of its target.**

### 5.1 Formal Statement

$$
\boxed{\forall\ a \in \mathcal{A} :\ \text{compatible}(\text{repd\_of}(a.\text{target}),\ \text{expected\_repd}(a))}
$$

where $\text{expected\_repd}(a)$ is the RepD required by the operation performed at access $a$ (e.g., a pointer dereference expects a RepD with pointer layout; an integer add expects an integer RepD).

**Definition of `compatible`:**

$$
\text{compatible}(r_1, r_2) \iff r_1.\text{size} = r_2.\text{size} \land r_1.\text{alignment} \mid \text{addr}(a.\text{target}) \land \text{valid\_reinterpretation}(r_1, r_2)
$$

where $\text{valid\_reinterpretation}$ is defined as:

$$
\text{valid\_reinterpretation}(r_1, r_2) \iff \begin{cases}
\text{true} & \text{if } r_1 = r_2 \text{ (same RepD)} \\
\text{true} & \text{if } r_1 \sqsubseteq r_2 \text{ (r1 is a sub-Repd of r2, e.g., bytes ⊑ any)} \\
\text{true} & \text{if cast is explicit and } r_2 \text{ is a valid union member} \\
\text{false} & \text{if } r_1 \in \text{PointerRepD} \land r_2 \notin \text{PointerRepD} \cup \text{BytesRepD} \\
\text{false} & \text{otherwise (needs IVE case analysis)}
\end{cases}
$$

**Uninitialized memory restriction:**

$$
\forall\ a \in \mathcal{A} :\ \text{expected\_repd}(a) \in \text{PointerRepD} \Rightarrow \text{is\_initialized}(\text{region\_of}(a.\text{target}),\ \text{bytes}(a))
$$

Reading uninitialized memory as a pointer type is always forbidden, because an arbitrary bit pattern interpreted as a pointer may point to an arbitrary address, violating Origin (Invariant 4).

### 5.2 Proof Strategy Sketch

1. **RepD propagation**: For each Derivation, compute $\text{repd\_of}(d)$ by walking the derivation chain. The most recent cast in the chain determines the effective RepD.

2. **Cast validation**: For each cast derivation $d$ (where $d.\text{cast} \neq \text{null}$), verify that the cast is a valid reinterpretation. This requires:
   - Size compatibility: the target RepD's size must not exceed the remaining bytes in the source region from the offset.
   - Alignment: the resolved address must satisfy the target RepD's alignment requirement.
   - Semantic compatibility: the IVE verifies that the bit pattern, when interpreted according to the target RepD, satisfies the invariants required by subsequent operations.

3. **Uninitialized memory tracking**: The IVE maintains an initialization map $\text{init\_map} : \text{Region} \times \text{ByteOffset} \to \{\text{Init}, \text{Uninit}\}$. After a Write access, the written bytes are marked Init. The IVE verifies that no Read access with a Pointer RepD targets Uninit bytes.

4. **Operation-RepD matching**: For each Access $a$, verify that the operation being performed is valid given $\text{repd\_of}(a.\text{target})$. For example:
   - Pointer dereference requires $\text{repd\_of}(a.\text{target}) \in \text{PointerRepD}$.
   - Floating-point operation requires a float RepD.
   - Byte copy accepts any RepD (bytes is the universal supertype).

5. **Transitivity of reinterpretation**: If a value is cast from RepD A to RepD B and then from B to C, the IVE must verify that A → B → C is a valid reinterpretation chain, not just that each individual step is valid. This prevents chains like `pointer → int → float` where the final interpretation is semantically unsound.

### 5.3 Example: Satisfying Program

```
// Valid cast: interpret bytes as header struct
r = allocate(bytes[1024]);           // Region r, default RepD: bytes[1024]
d1 = derive(r, offset=0);            // Derivation d1, RepD: bytes[1024]
d2 = derive(d1, offset=0,            // Cast derivation d2
            cast=Header);             //   RepD: Header { magic: uint32, size: uint32 }
a = read(d2, size=8);                // Access a: Read, expected_repd=Header

// Verification:
//   repd_of(d2) = Header (from cast)
//   compatible(Header, Header): same RepD ✓
//   addr(d2) = r.base_addr, alignment(Header)=4, r.base_addr % 4 = 0 ✓
//   sizeof(Header) = 8 ≤ 1024 ✓
// Interpretation invariant: SATISFIED
```

### 5.4 Example: Violating Program

```
// Reading uninitialized memory as a pointer
r = allocate(bytes[64]);             // Region r, all bytes Uninit
d1 = derive(r, offset=0);
d2 = derive(d1, offset=0,            // Cast derivation
            cast=ptr<u8>);            //   RepD: ptr<u8> (pointer type)
a = read(d2, size=8);                // Access a: Read, expected_repd=ptr<u8>

// Verification:
//   repd_of(d2) = ptr<u8>
//   expected_repd(a) = ptr<u8>
//   compatible(ptr<u8>, ptr<u8>): same RepD ✓
//   BUT: is_initialized(r, [0, 8)) = false (no prior Write to r)
//   Reading Uninit bytes as pointer: FORBIDDEN
// Interpretation invariant: VIOLATED
// IVE report: "Access a reads uninitialized bytes r[0..8) as pointer type ptr<u8>"
```

---

## 6. Invariant 4: Origin

> **Every address traces to a valid allocation; arithmetic derivations stay within bounds.**

### 6.1 Formal Statement

**Part A — Trace terminates at allocation:**

$$
\boxed{\forall\ d \in \mathcal{D} :\ \text{trace}(d) \neq \text{diverges}}
$$

$$
\text{trace}(d) = \begin{cases}
[r] & \text{if } d.\text{source} = r \in \mathcal{R} \\
\text{trace}(d.\text{source}) \mathbin{+\!\!+} [d] & \text{if } d.\text{source} \in \mathcal{D}
\end{cases}
$$

The trace always terminates because derivations form a forest (no cycles), and every tree root is a Region. Formally, the derivation graph is a DAG where all sinks are Regions.

**Part B — Arithmetic derivations stay in bounds:**

$$
\boxed{\forall\ d \in \mathcal{D} :\ d.\text{offset} \neq 0 \Rightarrow 0 \leq d.\text{offset} \land d.\text{offset} \leq \text{region\_of}(d).\text{size}}
$$

More precisely, for any derivation chain $[r, d_1, d_2, \ldots, d_n]$ where $d_n$ is the final derivation:

$$
\text{addr}(d_n) + \text{max\_access\_size}(d_n) \leq \text{region\_of}(d_n).\text{base\_addr} + \text{region\_of}(d_n).\text{size}
$$

where $\text{max\_access\_size}(d_n)$ is the maximum size of any access targeting $d_n$.

**Part C — No fabrication:**

$$
\forall\ a \in \mathcal{A} :\ \nexists\ \text{fabrication in trace}(a.\text{target})
$$

An address is "fabricated" if it is introduced by a computation that the IVE cannot trace to a valid allocation. Examples include: integer literals interpreted as addresses (`0xDEADBEEF`), addresses returned by FFI functions without derivation tracking, and addresses computed from uninitialized memory.

### 6.2 Proof Strategy Sketch

1. **DAG verification**: Verify that the derivation graph is acyclic. This is a structural property of the MSG — if the IVE constructs it correctly from the SCG, it is a DAG by construction. Detecting cycles is a simple graph traversal.

2. **Trace computation**: For each Derivation $d$, compute $\text{trace}(d)$ by walking the `source` chain. Termination is guaranteed by the DAG property. Each trace element records the offset and cast applied at that step.

3. **Bounds checking**: For each offset derivation $d$ with $d.\text{offset} \neq 0$, verify:
   - $d.\text{offset} \geq 0$ (no negative offsets without explicit signed arithmetic — if signed, verify it doesn't go below the region base).
   - The cumulative offset from the root Region's base address does not exceed the Region's size.

4. **Fabrication detection**: For each derivation, verify that its source is either a Region or another Derivation — never a raw integer constant or an untracked external value. If the program contains FFI calls that return addresses, the IVE must either:
   - Treat the returned address as a new Region (with the FFI call as the allocation point), or
   - Mark the address as unproven and flag it for review.

5. **Alias analysis**: If two different derivation chains produce the same address, verify that they trace to the same Region. Different Regions occupying overlapping addresses (in non-overlapping lifetimes) are acceptable; different Regions occupying overlapping addresses in overlapping lifetimes may indicate an Origin violation.

### 6.3 Example: Satisfying Program

```
// Valid pointer arithmetic within bounds
r = allocate(bytes[1024]);         // Region r: base=0x1000, size=1024
d1 = derive(r, offset=64);         // addr(d1) = 0x1040
d2 = derive(d1, offset=128);       // addr(d2) = 0x10C0
d3 = derive(d2, offset=0,          // Cast at d2's address
            cast=Header);           // RepD: Header (size=8)
a = read(d3, size=8);              // Access bytes [0x10C0, 0x10C8)

// Verification:
//   trace(d3) = [r, d1, d2, d3]
//   All sources are Region or Derivation: no fabrication ✓
//   Cumulative offset: 64 + 128 + 0 = 192
//   addr(d3) + 8 = 0x10C8 ≤ 0x1000 + 1024 = 0x1400 ✓
// Origin invariant: SATISFIED
```

### 6.4 Example: Violating Program

```
// Fabricated address from integer literal
r = allocate(bytes[1024]);         // Region r: base=0x1000, size=1024
d = derive_from_integer(0xDEADBEEF); // Fabrication: source is not a Region or Derivation
a = read(d, size=4);               // Access at 0xDEADBEEF

// Verification:
//   trace(d) does not terminate at a Region
//   d.source is an integer literal, not a valid derivation source
// Origin invariant: VIOLATED
// IVE report: "Address 0xDEADBEEF has no traceable origin to a valid allocation"
```

---

## 7. Invariant 5: Cleanup

> **Every allocation is eventually freed or explicitly leaked; no region is freed twice.**

### 7.1 Formal Statement

**Part A — Every region is freed or explicitly leaked:**

$$
\boxed{\forall\ r \in \mathcal{R} :\ r.\text{free\_point} \neq \text{null} \lor r.\text{status} = \text{Leaked}}
$$

where $\text{Leaked}$ is an explicit annotation applied by the programmer or inferred by the IVE to indicate intentional non-deallocation (e.g., long-lived arenas, global state, process-lifetime mappings).

**Part B — No double-free:**

$$
\boxed{\forall\ r \in \mathcal{R} :\ \text{count\_free}(r) \leq 1}
$$

where $\text{count\_free}(r) = |\{pp \in \text{PP} : \text{free\_op}(pp, r)\}|$, and $\text{free\_op}(pp, r)$ means a free operation targeting $r$ occurs at program point $pp$.

**Part C — Freed regions are not accessed (temporal safety, overlaps Liveness):**

$$
\forall\ r \in \mathcal{R},\ \forall\ a \in \mathcal{A} :\ \text{region\_of}(a.\text{target}) = r \land r.\text{free\_point} = pp_f \Rightarrow a.\text{program\_point} <_{pp} pp_f
$$

This is a restatement of the Liveness invariant (Invariant 1) but from the perspective of the region's lifecycle rather than the access's perspective. It is included here for completeness.

### 7.2 Proof Strategy Sketch

1. **Leak detection**: For each Region $r$ where $r.\text{free\_point} = \text{null}$:
   - If $r.\text{status} = \text{Stack}$, the region is implicitly freed when the stack frame is deallocated. The IVE verifies that the stack frame deallocation point is reachable.
   - If $r.\text{status} = \text{Allocated}$, the IVE checks whether $r$ is explicitly marked $\text{Leaked}$. If not, it searches for a feasible execution path where the program terminates or the region goes out of scope without deallocation.
   - If $r.\text{status} = \text{Mapped}$, the IVE verifies that the mapping is revoked before program termination or that it is explicitly marked $\text{Leaked}$.

2. **Double-free detection**: For each Region $r$, verify that at most one program point executes a free operation on $r$. This requires:
   - Control flow analysis: verify that no execution path reaches two different free operations on the same region.
   - Alias analysis: verify that two free operations on different derivation chains do not target the same region (i.e., `region_of(d1) = region_of(d2)` where both are freed).

3. **Ownership tracking (optional strengthening)**: The IVE may track an "ownership set" for each Region — the set of derivation chains that have the authority to free the region. Only derivations in the ownership set may initiate a free. This prevents accidental double-free through aliasing.

4. **Path-sensitive analysis**: For programs with conditional deallocation (e.g., `if (condition) free(r);`), the IVE must verify that:
   - On all paths where the condition is true, the free occurs exactly once.
   - On all paths where the condition is false, either the region is Leaked or it is freed on another path.

5. **Leaked annotation verification**: If a region is marked $\text{Leaked}$, the IVE verifies that the annotation is justified:
   - The region is reachable from a global root (arena, static variable).
   - The region's lifetime is intended to span the entire process.
   - No dangling references to the region exist after the process terminates (trivially true at process exit).

### 7.3 Example: Satisfying Program

```
// Arena allocation with explicit leak annotation
arena = allocate(bytes[65536]);     // Region arena, mark as Leaked (global arena)
r1 = allocate_from(arena, 64);      // Sub-region r1, within arena
r2 = allocate_from(arena, 128);     // Sub-region r2, within arena
free(r1);                           // r1.free_point = PP1
free(r2);                           // r2.free_point = PP2
// arena: free_point = null, status = Leaked ✓

// Verification:
//   r1.free_point = PP1 ≠ null ✓
//   r2.free_point = PP2 ≠ null ✓
//   arena: Leaked annotation, justified as global arena ✓
//   count_free(r1) = 1 ✓
//   count_free(r2) = 1 ✓
// Cleanup invariant: SATISFIED
```

### 7.4 Example: Violating Program

```
// Memory leak and double-free
r = allocate(bytes[256]);          // Region r: alloc_point=PP1
// ... no free(r) on any execution path ...

// OR:

r = allocate(bytes[256]);          // Region r: alloc_point=PP1
free(r);                           // r.free_point = PP2
free(r);                           // Second free at PP3

// Verification (leak case):
//   r.free_point = null, r.status ≠ Leaked
//   No reachable free operation on any execution path
// Cleanup invariant: VIOLATED (leak)
// IVE report: "Region r allocated at PP1 is never freed and not marked Leaked"

// Verification (double-free case):
//   count_free(r) = 2 (at PP2 and PP3)
// Cleanup invariant: VIOLATED (double-free)
// IVE report: "Region r freed at PP2 and again at PP3"
```

---

## 8. Invariant Dependency Graph

The five invariants are not independent. The following diagram shows their logical dependencies:

```
    ┌──────────┐
    │  Origin  │  (Invariant 4)
    │   (4)    │
    └────┬─────┘
         │ region_of() is well-defined
         ▼
    ┌──────────┐
    │ Liveness │  (Invariant 1)
    │   (1)    │
    └────┬─────┘
         │ region is allocated at access point
         ▼
    ┌──────────────┐     ┌────────────────┐
    │ Exclusivity  │────▷│ Interpretation │
    │     (2)      │     │      (3)       │
    └──────────────┘     └────────────────┘
         │                      │
         │ write doesn't        │ RepD is valid
         │ conflict             │
         ▼                      ▼
    ┌──────────┐
    │  Cleanup │  (Invariant 5)
    │   (5)    │
    └──────────┘
```

**Dependency explanation:**

| Dependency | Reason |
|-----------|--------|
| Liveness → Origin | `region_of()` (used by Liveness) is well-defined only if every derivation traces to a Region (Origin). |
| Exclusivity → Liveness | Exclusivity checks overlap on allocated regions; checking exclusivity on freed memory is meaningless. |
| Interpretation → Liveness | Reading a RepD from unallocated memory is a Liveness violation before it can be an Interpretation violation. |
| Interpretation → Exclusivity | A Write that sets up a valid RepD must be exclusive with Reads that interpret it; otherwise the RepD may be observed in an inconsistent state. |
| Cleanup → Liveness, Exclusivity | Cleanup is a temporal property: it ensures that the "allocated" status in Liveness eventually transitions to "freed." Cleanup depends on Liveness to define when a region is live. |

**Proof order**: The IVE should verify invariants in topological order: Origin → Liveness → (Exclusivity, Interpretation) → Cleanup.

---


---

## 9. Formal Theorems

The previous sections state each of the five invariants formally (§§3.1, 4.1, 5.1, 6.1, 7.1) and §8 sketches their *dependency* graph (one invariant can be checked only after another has been established). This section establishes the *independence* of the invariants — a complementary result showing that none is redundant.

### 9.1 Formal Definitions

Let $\mathcal{I}_1, \mathcal{I}_2, \mathcal{I}_3, \mathcal{I}_4, \mathcal{I}_5$ denote the predicates on MSGs corresponding to **Liveness**, **Exclusivity**, **Interpretation**, **Origin**, and **Cleanup** respectively (formal statements in §§3.1, 4.1, 5.1, 6.1, 7.1). Let $\mathsf{MSGs}$ denote the universe of well-typed MSGs — tuples $(\mathcal{R}, \mathcal{D}, \mathcal{A}, \mathcal{S})$ whose components satisfy the type constraints of §2. For a predicate $P$ on MSGs, let
$$
\llbracket P \rrbracket = \{ M \in \mathsf{MSGs} \mid P(M) \}
$$
denote its satisfying set. Two invariants $P, Q$ are **equivalent** ($P \equiv Q$) iff $\llbracket P \rrbracket = \llbracket Q \rrbracket$; $P$ **implies** $Q$ ($P \Rightarrow Q$) iff $\llbracket P \rrbracket \subseteq \llbracket Q \rrbracket$. The invariants are **logically independent** iff for every $i$, $\bigcap_{j \neq i} \llbracket \mathcal{I}_j \rrbracket \not\subseteq \llbracket \mathcal{I}_i \rrbracket$.

### 9.2 Theorem (Invariant Independence)

**The five invariants are logically independent: no invariant is implied by the conjunction of the other four.** Formally, for each $i \in \{1, 2, 3, 4, 5\}$,
$$
\bigcap_{j \neq i} \llbracket \mathcal{I}_j \rrbracket \;\not\subseteq\; \llbracket \mathcal{I}_i \rrbracket.
$$

**Proof sketch.** By explicit counterexample MSG $M_i$ for each $i$, in which every $\mathcal{I}_j$ with $j \neq i$ holds but $\mathcal{I}_i$ fails. Each $M_i$ is a witness that $\bigcap_{j \neq i} \llbracket \mathcal{I}_j \rrbracket \not\subseteq \llbracket \mathcal{I}_i \rrbracket$.

- **$M_1$ (Liveness fails, rest hold) — use-after-free.** Region $r$ allocated at PP1, freed at PP2; access $a$ targeting $r$ at PP3 with $\text{PP2} <_{pp} \text{PP3}$.
  - $\mathcal{I}_1$ **fails**: $\text{is\_allocated}(r, \text{PP3})$ is false because $r.\text{free\_point} = \text{PP2} \leq \text{PP3}$ (§3.1).
  - $\mathcal{I}_2$ **holds**: only one access, so $\text{conflicts}$ is vacuously false.
  - $\mathcal{I}_3$ **holds**: the bytes at $a$'s offset within $r$ carry a valid RepD from before the free (the bytes are not erased by deallocation; the *interpretation* check is about the byte layout, not liveness).
  - $\mathcal{I}_4$ **holds**: $\text{region\_of}(a.\text{target}) = r$, a valid region.
  - $\mathcal{I}_5$ **holds**: $r$ has a valid $\text{free\_point}$ at PP2 (single free, no leak).

- **$M_2$ (Exclusivity fails, rest hold) — data race.** Region $r$ allocated at PP1, live throughout. Two Write accesses $a_1, a_2$ on concurrent paths $\pi_1, \pi_2$ (so $\text{ordered}(a_1, a_2)$ is false), targeting overlapping byte ranges of $r$.
  - $\mathcal{I}_1$ **holds**: $r$ is live at both access points.
  - $\mathcal{I}_2$ **fails**: $\text{conflicts}(a_1, a_2)$ (two Writes, overlapping ranges, no SyncEdge) and $\neg\text{ordered}(a_1, a_2)$.
  - $\mathcal{I}_3$ **holds**: both Writes carry valid RepDs.
  - $\mathcal{I}_4$ **holds**: both accesses target valid derivations rooted at $r$.
  - $\mathcal{I}_5$ **holds**: $r$ is later freed at PP4.

- **$M_3$ (Interpretation fails, rest hold) — type confusion.** Region $r$ allocated with $\text{RepD} = \text{bytes}[4]$; access $a$ reads 8 bytes at offset 0 interpreting them as a pointer (an 8-byte RepD), violating the layout.
  - $\mathcal{I}_1$ **holds**: $r$ is live at $a$'s program point.
  - $\mathcal{I}_2$ **holds**: single access.
  - $\mathcal{I}_3$ **fails**: $a$'s target RepD (8-byte pointer) does not match the region's stored RepD (4-byte); the bytes-interpretable-as-RepD condition fails.
  - $\mathcal{I}_4$ **holds**: derivation rooted at $r$.
  - $\mathcal{I}_5$ **holds**: $r$ is freed at end.

- **$M_4$ (Origin fails, rest hold) — dangling raw pointer.** An access $a$ whose derivation chain does not terminate at any region (e.g., a literal integer cast to a pointer with no allocation root). The byte range accessed falls within a coincidentally-live allocated region $r$.
  - $\mathcal{I}_1$ **holds**: $r$ is live at $a$'s program point, and $a$'s byte range is within $r$'s range — by construction, $a$ happens to land on allocated memory.
  - $\mathcal{I}_2$ **holds**: single access.
  - $\mathcal{I}_3$ **holds**: bytes are interpretable (we choose the access RepD to match).
  - $\mathcal{I}_4$ **fails**: $\text{region\_of}(a.\text{target})$ is undefined because the derivation chain has no root region.
  - $\mathcal{I}_5$ **holds**: $r$ is properly freed (the leaked derivation is not a region).

- **$M_5$ (Cleanup fails, rest hold) — memory leak.** Region $r$ allocated at PP1, never freed, not marked $\text{Leaked}$; one access $a$ to $r$ at PP2.
  - $\mathcal{I}_1$ **holds**: $r$ is live throughout the program (no $\text{free\_point}$), so every access is live.
  - $\mathcal{I}_2$ **holds**: single access.
  - $\mathcal{I}_3$ **holds**: bytes interpretable.
  - $\mathcal{I}_4$ **holds**: derivation rooted at $r$.
  - $\mathcal{I}_5$ **fails**: $r.\text{free\_point} = \text{null}$ and $r.\text{status} \neq \text{Leaked}$, so no $\text{free}$ operation on any execution path and no intentional-leak annotation.

Each $M_i$ is well-typed (its components satisfy §2) and is a witness to $\bigcap_{j \neq i} \llbracket \mathcal{I}_j \rrbracket \not\subseteq \llbracket \mathcal{I}_i \rrbracket$. Hence no invariant is implied by the conjunction of the other four; the five invariants are logically independent. $\square$

### 9.3 Theorem (Pairwise Non-Equivalence)

**Any two distinct invariants $\mathcal{I}_i, \mathcal{I}_j$ with $i \neq j$ are non-equivalent:** $\llbracket \mathcal{I}_i \rrbracket \neq \llbracket \mathcal{I}_j \rrbracket$.

**Proof sketch.** For the ordered pair $(i, j)$, the counterexample $M_i$ above (§9.2) satisfies $\mathcal{I}_j$ but not $\mathcal{I}_i$, witnessing $M_i \in \llbracket \mathcal{I}_j \rrbracket \setminus \llbracket \mathcal{I}_i \rrbracket$. Hence $\llbracket \mathcal{I}_i \rrbracket \neq \llbracket \mathcal{I}_j \rrbracket$. $\square$

### 9.4 Corollary (Minimal Invariant Set)

The set $\{\mathcal{I}_1, \ldots, \mathcal{I}_5\}$ is **minimal**: no proper subset entails all five invariants. By §9.2, removing any $\mathcal{I}_i$ admits the counterexample $M_i$, in which the remaining four hold but the safety property corresponding to $\mathcal{I}_i$ is violated. Therefore the IVE must verify all five; omitting any one would leave a soundness gap.

---

## 10. References

1. Proposal: "Beyond Human Syntax: A Proposal for AI-Native Programming Language Design" (Section 2.9: The Safety Through Restriction Fallacy; Section 3.6: Verified-Unsafe Memory Access).
2. VUMA Source: `vuma/src/vuma/src/address.rs` — Address type definition.
3. VUMA Source: `vuma/src/vuma/src/program_point.rs` — Program point type definition.
4. VUMA Source: `vuma/src/ive/src/constraint.rs` — Constraint inference infrastructure.
5. VUMA Source: `vuma/src/ive/src/debt.rs` — Verification debt tracking.
6. VUMA Source: `vuma/src/proof/src/` — Proof engine infrastructure.

---

*End of specification.*
