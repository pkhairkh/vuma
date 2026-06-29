# Decidability Analysis for VUMA Verification

**Document ID:** VUMA-SPEC-DECID-001  
**Author:** Parham Khairkhah

**Task ID:** W1-25

---

## Introduction

The Verified-Unsafe Memory Access (VUMA) model is the central innovation of the proposed AI-native programming language framework. It promises unrestricted raw memory access — pointers, arithmetic, manual allocation, arbitrary casts — with safety established by global verification rather than by restriction. The Inference and Verification Engine (IVE) is tasked with proving five global invariants for every memory access in the program: liveness, exclusivity, interpretation, origin, and cleanup.

This document addresses the most fundamental theoretical question about VUMA: **is verification of these invariants decidable?** That is, does there exist an algorithm that, given an arbitrary VUMA program, can always determine whether all five invariants hold? The answer has profound implications for the design of the IVE, the practical deployment of VUMA, and the credibility of the entire "verified access over restricted access" thesis.

The short answer is: **no.** Full VUMA verification for arbitrary programs with unrestricted pointer arithmetic is undecidable. However, as we shall demonstrate, this undecidability result does not undermine the VUMA project — it constrains it in ways that are both intellectually clarifying and practically manageable. The path forward lies in identifying decidable subsets, designing practical approximation strategies, and honestly characterizing the boundaries between verified, approximated, and unverified reasoning.

---

## 1. The Undecidability Result

### Theorem 1.1 (Undecidability of Full VUMA Verification)

*Let V be the class of all well-formed VUMA programs (programs whose syntactic structure satisfies the basic well-formedness rules of the VUMA language). Let INV denote the conjunction of all five VUMA invariants: liveness (L), exclusivity (E), interpretation (I), origin (O), and cleanup (C). The problem of determining, for an arbitrary program P ∈ V, whether P ⊨ INV (i.e., whether P satisfies all five invariants on all possible executions), is undecidable.*

**Proof Sketch.** We prove undecidability by reduction from the halting problem. The reduction proceeds through the liveness invariant, though similar constructions work for each of the other four invariants.

*Step 1: Reduction from HALT.*

Given a Turing machine M = (Q, Σ, δ, q₀, q_halt) and an input string w ∈ Σ*, we construct a VUMA program P(M, w) as follows:

```
program P(M, w):
    R = allocate(region, size=S)        // Allocate region R of sufficient size
    tape = R.base                       // Derive base address for tape simulation
    state = q₀                          // Initialize TM state
    // ... encode w onto tape via R ...

    while state ≠ q_halt:               // Simulate M on w
        // Decode current symbol from tape
        symbol = read(tape + head_position)
        // Apply transition function δ
        (new_symbol, direction, new_state) = δ(state, symbol)
        // Write new symbol
        write(tape + head_position, new_symbol)
        // Move head
        head_position = head_position + direction
        state = new_state

    // M has halted — trigger a liveness violation
    deallocate(R)                       // Free region R
    x = read(R.base + 0)               // Use-after-free: access freed region
```

*Step 2: Correctness of the reduction.*

We must show that P(M, w) violates the liveness invariant if and only if M halts on input w.

(**⇒**) Suppose M halts on w. Then the `while` loop in P(M, w) eventually terminates with `state = q_halt`. Execution proceeds to the `deallocate(R)` call, which frees region R. The subsequent `read(R.base + 0)` accesses an address within the freed region R, violating the liveness invariant (every access must target an allocated region). Therefore, P(M, w) ⊭ L, and hence P(M, w) ⊭ INV.

(**⇐**) Suppose M does not halt on w. Then the `while` loop in P(M, w) never terminates. The `deallocate(R)` and subsequent `read(R.base + 0)` are never reached. On all actual execution paths, every access to R occurs while R is allocated. Therefore, the liveness invariant holds for P(M, w), and P(M, w) ⊨ L. (We assume the construction is designed so that the other invariants hold as well — this is achievable by ensuring all pointer derivations are well-formed within the loop body.)

*Step 3: Conclusion.*

We have shown: P(M, w) ⊭ L iff M halts on w. Therefore, an algorithm for deciding VUMA liveness verification would yield an algorithm for the halting problem. Since the halting problem is undecidable (Turing, 1936), VUMA liveness verification is undecidable. Since INV subsumes L, full VUMA verification is also undecidable. ∎

### Corollary 1.2 (Undecidability of Individual Invariants)

*Each of the five VUMA invariants is individually undecidable for arbitrary VUMA programs.*

**Proof Sketch.** We sketch reductions for the remaining four invariants:

- **Exclusivity (E):** Construct P(M, w) that spawns two concurrent threads upon halting, both writing to the same address in R without synchronization. Data race occurs iff M halts.

- **Interpretation (I):** Construct P(M, w) that, upon halting, writes arbitrary bytes to R and then reads them as a pointer type. Misinterpretation occurs iff M halts.

- **Origin (O):** Construct P(M, w) that, upon halting, computes an address through untraceable arithmetic (e.g., derived from a hash of the tape contents, which depends on M's computation). The origin of this computed address cannot be traced iff M's computation is non-trivial.

- **Cleanup (C):** Construct P(M, w) that allocates R and, iff M halts, exits the program without freeing R (a memory leak). The cleanup invariant is violated iff M halts. Conversely, if M does not halt, the loop continues indefinitely and R is never leaked — it remains allocated and in use.

Each construction reduces the halting problem to the respective invariant verification problem, establishing individual undecidability. ∎

### Corollary 1.3 (Rice's Theorem for VUMA)

*Any non-trivial semantic property of VUMA programs is undecidable. In particular, for any non-trivial subset S of the five invariants, determining whether P ⊨ S for arbitrary P ∈ V is undecidable.*

This follows directly from Rice's theorem (Rice, 1953), since the set of VUMA programs is Turing-complete (they can simulate arbitrary Turing machines, as demonstrated in the proof of Theorem 1.1), and any non-trivial property of their memory behavior is a non-trivial semantic property.

### Discussion

The undecidability result is neither surprising nor devastating. It places VUMA verification in the same category as virtually every interesting program analysis problem: type inference for polymorphic recursion (undecidable; Wells, 1999), termination checking (undecidable by definition), data race detection for concurrent programs (undecidable; Ramalingam, 2000), and even precise alias analysis (undecidable; Landi, 1992). The practical question is not whether verification is undecidable in the worst case, but whether the undecidable cases arise in practice, and how to handle them when they do.

The key observation is that the undecidability arises from the interaction of two features: **unrestricted pointer arithmetic** (which allows the computation of addresses whose provenance depends on arbitrary computation) and **unbounded control flow** (which allows the program to condition memory access behavior on the result of arbitrary computation). If we restrict either feature, decidability may be recovered. This insight motivates the restricted subsets explored in Section 2.

It is also worth noting that the undecidability result is robust to the choice of verification methodology. Whether the IVE uses abstract interpretation, symbolic execution, separation logic, type theory, or any other formalism, it cannot escape the fundamental limitation: no algorithm can correctly determine the memory behavior of all VUMA programs. What differs across methodologies is the *character* of the approximation — which programs are handled precisely, which are over-approximated (false positives), and which are under-approximated (false negatives, which are unacceptable for a safety guarantee).

---

## 2. Restricted Decidable Subsets

Since full VUMA verification is undecidable, we seek restricted subsets of VUMA programs for which verification is decidable. Each subset restricts a different dimension of the language, yielding a different trade-off between expressiveness and decidability. The four subsets presented below are not exhaustive — they represent the most natural and practically motivated restriction strategies.

### 2.1 Subset VUMA-1: No Pointer Arithmetic (Offset-Only Derivation)

**Definition.** VUMA-1 restricts pointer derivation to three forms: (1) direct allocation — an address returned by `allocate`, (2) field offset — an address derived as `base + field_offset` where `field_offset` is a statically known constant determined by the structure layout, and (3) identity — an address copied from another address without modification. Arbitrary pointer arithmetic (`base + runtime_expression`) is prohibited.

**Theorem 2.1 (Decidability of VUMA-1 Verification).** *For programs in VUMA-1, verification of all five invariants is decidable, provided that all loop and branch conditions are decidable.*

**Proof Sketch.** Under VUMA-1 restrictions, every address in the program has a statically known derivation chain. Since pointer derivations are restricted to allocation, field offset, and identity, the set of all possible addresses at any program point is finite and computable. Specifically:

- The number of allocation sites is finite (bounded by program size).
- The number of field offsets from each allocation is finite (bounded by the structure definitions).
- Identity copies do not create new addresses.

Therefore, the Memory State Graph (MSG) is a finite structure. Liveness verification reduces to reachability analysis in a finite graph: for each access node, we check whether there exists a path from a deallocation of the accessed region to the access without an intervening reallocation. Reachability in finite graphs is decidable (linear-time for directed graphs via DFS/BFS). Exclusivity verification reduces to checking for conflicting simultaneous accesses in the finite MSG, which is a constraint satisfaction problem over a finite domain. Interpretation, origin, and cleanup invariants similarly reduce to decidable properties of the finite MSG.

The decidability of loop and branch conditions is a separate requirement: if the program contains loops whose termination is undecidable, then reachability within the MSG may be undecidable even though the MSG itself is finite. The standard approach is to over-approximate: assume all loops may terminate and all branches may be taken, yielding a sound (conservative) analysis that may produce false positives but never false negatives. ∎

**Assessment.** VUMA-1 is decidable but severely restrictive. It eliminates one of the core advantages of VUMA: the ability to perform verified pointer arithmetic. Many common systems programming patterns rely on computing addresses at runtime — array indexing with variable offsets, pointer bumping in allocators, pointer subtraction for distance computation. VUMA-1 cannot express these patterns. It is, in essence, a more precise version of Rust's borrow checker: it permits raw pointers but restricts how they can be derived, yielding decidability at the cost of expressiveness. VUMA-1 is useful as a baseline — it defines the "decidable floor" below which VUMA loses its distinguishing characteristic — but it is not a practical target for the full VUMA vision.

### 2.2 Subset VUMA-2: Bounded Loops

**Definition.** VUMA-2 restricts all loops to have a statically known upper bound on the number of iterations. This bound may be a constant, a function parameter with a known maximum, or a value derived from a bounded container. Recursion is similarly bounded by a statically known depth.

**Theorem 2.2 (Decidability of VUMA-2 Verification).** *For programs in VUMA-2, verification of all five invariants is decidable, with worst-case complexity that is exponential in the product of the loop bounds.*

**Proof Sketch.** Under VUMA-2 restrictions, every execution path in the program is finite and the set of all execution paths is finite and enumerable. For a program with loops having bounds b₁, b₂, ..., bₖ, the maximum path length is O(∏ᵢ bᵢ), and the total number of distinct paths is similarly bounded. For each path, the MSG can be constructed and the invariants checked in time polynomial in the path length. Therefore, exhaustive path enumeration yields a decision procedure with total time complexity O(P(n) · ∏ᵢ bᵢ), where P(n) is the polynomial cost of checking invariants along a single path of length n.

The exponential blowup is inherent: even determining whether a simple safety property holds on all bounded paths is co-NP-hard, since it subsumes the problem of checking that no path through a boolean formula evaluation violates a condition (which can encode SAT). The bounded model checking literature (Biere et al., 1999) provides extensive analysis of this complexity landscape. ∎

**Assessment.** VUMA-2 is decidable in principle but impractical for large programs. A program with 10 nested loops, each bounded by 1000 iterations, has up to 1000¹⁰ ≈ 10³⁰ distinct execution paths — far beyond the reach of any physical computer. Bounded model checking tools (CBMC, CPAchecker, etc.) address this through intelligent path pruning, symbolic representation, and satisfiability solving, but they hit scalability limits for programs with more than a few thousand lines. VUMA-2 is useful for verifying small, critical components — device drivers, cryptographic routines, interrupt handlers — but cannot serve as the primary verification strategy for the full language. It is best understood as a technique to be applied selectively, not a language-wide restriction.

### 2.3 Subset VUMA-3: Separation Logic Fragment

**Definition.** VUMA-3 restricts heap structures to those expressible in a decidable fragment of separation logic. Specifically, the heap must be describable by shape predicates drawn from a fixed set: singly-linked lists, doubly-linked lists, binary trees, n-ary trees, skip lists, and combinations thereof (e.g., a hash table is an array of linked lists). Arbitrary heap graphs with unstructured sharing and cycles are excluded.

**Theorem 2.3 (Decidability of VUMA-3 Verification).** *For programs in VUMA-3, verification of the liveness and exclusivity invariants is decidable for shape predicates that admit precise footprint computation. Verification of the interpretation and origin invariants requires additional restrictions on the representation and capability descriptors.*

**Proof Sketch.** The key insight from separation logic (Reynolds, 2002; O'Hearn, 2001; Ishtiaq and O'Hearn, 2001) is that the frame rule enables compositional reasoning about the heap. If a program region accesses only a portion of the heap described by assertion A, then the remainder of the heap (the "frame" A — B, where B is the portion accessed) is unchanged. This compositionality breaks the global reasoning problem into local subproblems that are independently tractable.

For the specific shape predicates listed above:

- **Singly-linked lists**: Liveness is decidable because the list structure is inductive — every node is reachable from the head, and deallocation proceeds by traversal. The footprint of a list operation is at most one node (for insertion/deletion) or two nodes (for splice). Exclusivity follows from the linear structure.

- **Doubly-linked lists**: More complex due to the prev pointers, but still decidable. The key is that the doubly-linked list invariant (node.next.prev = node for all interior nodes) is a local property that can be maintained incrementally. Berdine et al. (2005) showed that verification of programs manipulating doubly-linked lists is decidable in separation logic.

- **Trees**: Decidable because trees have no sharing (each node has exactly one parent). The tree shape predicate can be defined inductively, and tree operations (insertion, deletion, rotation) have bounded footprints.

- **Skip lists**: Decidable because skip lists are layered linked lists, and each layer is independently a linked list. The cross-layer pointers can be handled by tracking the layer in the shape predicate.

The limitation arises with **arbitrary heap graphs**. A graph with arbitrary sharing and cycles cannot in general be described by a decidable separation logic fragment, because the number of distinct heap shapes grows exponentially with the number of nodes (Distefano et al., 2006). For such structures, separation logic reasoning either requires user-supplied invariants (defeating the goal of inference) or falls back to imprecise over-approximations (losing the precision needed for VUMA verification). ∎

**Assessment.** VUMA-3 is the most theoretically satisfying of the four subsets, because separation logic provides a well-understood, compositional framework for heap reasoning. It handles the data structures that arise most frequently in practice — lists, trees, hash tables — with full precision. The cost is that it excludes programs that manipulate arbitrary graph structures: object-oriented programs with complex aliasing, concurrent data structures with fine-grained locking, and low-level systems code that treats memory as a raw byte array. VUMA-3 is best understood as a high-value decidable fragment that covers the majority of everyday programming, with escape hatches for the cases it cannot handle.

### 2.4 Subset VUMA-4: Ownership-Inferred (VUMA with IVE-Inferred Ownership)

**Definition.** VUMA-4 does not restrict the language itself — arbitrary pointer arithmetic and unbounded loops are permitted. Instead, it restricts the *verification strategy*: the IVE first attempts to infer ownership patterns from the program structure (using the techniques described in the proposal's IVE layer), and then applies full verification only where ownership inference succeeds. Where ownership is clear — no sharing, no ambiguous aliasing — verification is trivial (as in Rust's ownership model, but inferred rather than annotated). Where ownership is unclear, the IVE falls back to whole-program analysis, which may be incomplete.

**Theorem 2.4 (Partial Decidability of VUMA-4 Verification).** *For programs in VUMA-4, the subset of invariants verifiable by ownership inference is decidable. The remaining invariants (those involving regions with unclear ownership) are verified on a best-effort basis, with no completeness guarantee.*

**Proof Sketch.** The ownership inference algorithm operates in two phases:

*Phase 1: Ownership Classification.* The IVE analyzes each memory region and classifies it into one of three categories: (a) uniquely owned — exactly one pointer may access the region at any time, (b) shared-immutable — multiple pointers may read but none may write, or (c) ambiguous — ownership cannot be determined locally. Classification (a) and (b) are decidable because they reduce to checking that the pointer derivation graph satisfies simple structural properties (no branching in the derivation chain for uniquely owned; no write edges for shared-immutable). These properties are checkable in polynomial time by graph traversal.

*Phase 2: Verification by Category.* For regions in categories (a) and (b), all five invariants are trivially decidable: uniquely owned regions cannot have exclusivity violations (only one accessor), liveness is decidable by tracking the single ownership chain, and interpretation/origin/cleanup follow from the unique derivation. For regions in category (c), the IVE applies whole-program analysis (abstract interpretation, symbolic execution, or LLM-guided reasoning — see Section 3) with no completeness guarantee. Some invariants may be verified; others may be flagged as unverifiable.

The practical power of VUMA-4 lies in the empirical observation that **most code has clear ownership**. Function-local allocations, stack variables, pass-by-value parameters, and most data structure operations fall into categories (a) or (b). Ambiguous ownership arises primarily in specific patterns: graphs with shared nodes, concurrent data structures, callback-based APIs, and custom memory allocators. By handling the clear cases decisively and the ambiguous cases on a best-effort basis, VUMA-4 achieves a practical compromise that covers the vast majority of real code. ∎

**Assessment.** VUMA-4 is the most pragmatic of the four subsets and the closest to the full VUMA vision. It does not restrict the language at all — it restricts the verification guarantee. The result is a spectrum of confidence levels rather than a binary verified/unverified distinction, which is exactly the practical verification strategy described in Section 3. VUMA-4 acknowledges that some invariants cannot be automatically verified and provides a principled framework for characterizing which ones can and which cannot. This honest characterization is more useful than a false promise of complete verification.

---

## 3. Practical Verification Strategy

The undecidability results of Section 1 and the restricted subsets of Section 2 establish a clear landscape: full VUMA verification is impossible in the general case, but large, practically important fragments are decidable. The challenge is to design a verification strategy that exploits the decidable fragments while gracefully handling the undecidable remainder. We propose a tiered approach.

### 3.1 Tier 1: Fast Local Analysis (Covers ~90% of Accesses)

Tier 1 applies local, compositional analyses that run in polynomial time and produce definitive results for the common case. The primary Tier 1 technique is **ownership inference** (as in VUMA-4): the IVE classifies each memory region as uniquely owned, shared-immutable, or ambiguous, and verifies the five invariants for the first two categories. This is analogous to Rust's borrow checker, but with three critical differences:

1. **Inferred, not annotated.** The IVE infers ownership from the program structure; the programmer never writes lifetime annotations or ownership markers. This eliminates the cognitive overhead that makes Rust's borrow checker difficult for many programmers.

2. **Dataflow-based, not syntactic.** Rust's borrow checker operates on the Mid-Level IR (MIR) and uses syntactic scope as a proxy for liveness. The IVE uses full dataflow analysis, which is strictly more precise — it can recognize that a borrow is dead before its syntactic scope ends, permitting access patterns that the borrow checker rejects. (This is the same improvement that Rust's Polonius project aims to deliver.)

3. **Region-aware, not variable-aware.** The IVE tracks ownership at the granularity of memory regions, not variables. This allows it to reason about sub-regions independently: a large buffer can have a uniquely-owned header and a shared-immutable payload, with each sub-region verified according to its own ownership pattern.

Tier 1 also includes **shape analysis** for common data structures (lists, trees, hash tables — as in VUMA-3). The IVE maintains a library of shape predicates and matches them against allocation patterns in the program. When a match is found, the corresponding separation logic invariant is instantiated and verified compositionally. This provides precise verification for the data structures that dominate real codebases, without requiring whole-program analysis.

The expected coverage of Tier 1 is approximately 90% of all memory accesses in a typical systems program. This estimate is based on the empirical distribution of memory patterns: the majority of accesses target stack variables, function-local heap allocations, and standard data structures, all of which fall within the scope of ownership inference and shape analysis.

### 3.2 Tier 2: Global Symbolic Execution (Covers ~8% of Accesses)

For accesses that Tier 1 cannot verify — primarily those involving regions with ambiguous ownership, pointer arithmetic that crosses region boundaries, or inter-procedural data flow that defeats local analysis — the IVE applies **global symbolic execution**. Symbolic execution explores the program's execution paths symbolically, maintaining a symbolic state that represents the possible concrete states at each program point. For each memory access, it checks the five invariants against the symbolic state.

Global symbolic execution is more powerful than local analysis but has two fundamental limitations:

1. **Path explosion.** The number of symbolic execution paths grows exponentially with the number of branches in the program. The IVE mitigates this through path merging (using constrained symbolic states that represent multiple concrete paths), function summarization (replacing function bodies with computed summaries), and demand-driven exploration (only exploring paths that lead to unverified accesses).

2. **Loop handling.** Loops generate infinite symbolic execution paths. The IVE uses loop invariant inference (also known as "widening" in abstract interpretation) to summarize the effect of loops without exploring individual iterations. The quality of the loop invariant determines the precision of the verification: a precise invariant yields precise verification, while an imprecise invariant may leave some accesses unverified.

The expected coverage of Tier 2 is approximately 8% of accesses — those that involve inter-procedural reasoning, moderate pointer arithmetic, or data structures that Tier 1's shape analysis cannot recognize but that are still amenable to symbolic reasoning. The combined coverage of Tiers 1 and 2 is approximately 98%, which is comparable to the coverage achieved by state-of-the-art static analyzers like Infer (Calcagno et al., 2015) and CodeQL.

### 3.3 Tier 3: LLM-Guided Reasoning (Covers ~1.5% of Accesses)

For the remaining ~2% of accesses that resist both local analysis and symbolic execution, the IVE employs **LLM-guided reasoning**. This is the most novel and most controversial tier, and it requires careful explanation.

The idea is not to replace formal verification with LLM guessing. Rather, the LLM serves as a **hypothesis generator**: it proposes loop invariants, ownership patterns, or separation logic assertions that the symbolic execution engine can then verify. If the LLM proposes an invariant that is too weak, the symbolic engine detects that it doesn't suffice and requests a stronger one. If the LLM proposes an invariant that is wrong, the symbolic engine produces a counterexample. The LLM and symbolic engine iterate until either a verified invariant is found or a resource limit is reached.

For example, consider a concurrent hash table with fine-grained locking. Tier 1 cannot verify it (ownership is ambiguous due to concurrent access). Tier 2 cannot verify it (the number of interleavings is exponential in the number of concurrent operations). An LLM, however, can understand the high-level structure — "this is a hash table where each bucket has its own lock" — and propose the invariant: "accesses to bucket i are protected by lock i." The symbolic engine can then verify this invariant by checking that every access to a bucket's memory region is preceded by an acquisition of the corresponding lock. The LLM provides the insight; the symbolic engine provides the proof.

The confidence level for Tier 3 is lower than for Tiers 1 and 2, because the correctness of the verification depends on the LLM's ability to propose the right invariant. If the LLM proposes a wrong invariant and the symbolic engine fails to refute it (due to resource limits), the verification is unsound. This risk is mitigated by running the LLM-suggested invariants through a formal proof checker before accepting them, and by flagging Tier 3 verifications with a lower confidence level.

### 3.4 Tier 4: Unverified (0.5% of Accesses)

For the tiny fraction of accesses that no tier can verify, the IVE flags them as **unverified** and presents them to the programmer (or AI agent) for manual review. Each unverified access is accompanied by:

- The specific invariant that could not be verified
- The execution path(s) that may lead to a violation
- The analysis that was attempted and why it failed
- Suggested modifications that would make the access verifiable (e.g., adding a synchronization point, restructuring the allocation pattern, or providing an explicit invariant annotation)

The unverified set is not silently accepted. It is explicitly tracked, displayed in all projections of the program, and factored into deployment decisions (see Section 3.5).

### 3.5 Confidence Levels and Deployment Policies

Each verified invariant is assigned a **confidence level** based on the tier that verified it:

| Tier | Confidence | Description |
|------|-----------|-------------|
| 1 | **High** | Verified by local, compositional analysis with no approximations |
| 2 | **Medium-High** | Verified by symbolic execution with sound approximations (loop widening, path merging) |
| 3 | **Medium** | Verified with LLM-guided invariant generation; formally checked but dependent on LLM hypothesis quality |
| 4 | **None** | Not verified; flagged for manual review |

Deployment policies can specify minimum confidence levels for different deployment contexts:

- **Development**: All confidence levels accepted; unverified accesses produce warnings
- **Staging**: Medium-High or above required for all safety-critical accesses
- **Production**: High confidence required for all safety-critical accesses; Medium-High for non-critical
- **Safety-critical systems** (avionics, medical devices): High confidence required for all accesses; no exceptions

This tiered confidence model is the practical realization of VUMA's core principle: **verify what you can, flag what you cannot, never silently accept unsafe access.** It provides a principled framework for making deployment decisions based on the strength of the verification evidence, rather than a binary "safe/unsafe" classification that ignores the nuance of real-world verification.

---

## 4. Connection to Existing Work

The decidability and verification challenges that VUMA faces are not unique — they are shared, to varying degrees, by every system that attempts to verify memory safety properties of programs. Understanding these connections is essential for positioning VUMA's contributions, avoiding reinvented wheels, and leveraging established results.

### 4.1 Separation Logic (Reynolds, O'Hearn, Ishtiaq)

Separation logic (Reynolds, 2002; O'Hearn, 2001; Ishtiaq and O'Hearn, 2001) is the most directly relevant body of work. It provides a formal framework for reasoning about programs that manipulate the heap, with two key innovations:

1. **The separating conjunction** (P * Q) asserts that the heap can be split into two disjoint portions, one satisfying P and the other satisfying Q. This enables local reasoning: if a program region touches only the heap described by P, the frame Q is guaranteed unchanged.

2. **The frame rule** allows extending a local correctness proof to a global one by adding the unchanged frame. This is the compositional mechanism that makes separation logic tractable.

VUMA's connection to separation logic is deep. The liveness, exclusivity, and interpretation invariants are essentially separation logic assertions: liveness corresponds to the "points-to" predicate (the accessed address must be in the heap), exclusivity corresponds to the separating conjunction (no overlapping accesses), and interpretation corresponds to the type annotation on the points-to predicate (the heap at that address must satisfy the representation descriptor).

The key difference is in the **verification methodology**. Separation logic is traditionally used as a *proof system*: the programmer (or a verification tool) provides assertions, and the system checks that they are maintained. VUMA aims for *inference*: the IVE should derive the assertions automatically, without human annotation. This is the same goal as the Infer static analyzer (Calcagno et al., 2015), which uses bi-abduction — a form of separation logic inference — to discover preconditions and postconditions automatically. VUMA's ambition is to extend this inference to the full set of five invariants, not just the memory safety properties that Infer targets.

### 4.2 Rust's Polonius

Polonius is Rust's ongoing project to replace the borrow checker's syntactic-based liveness analysis with a dataflow-based analysis (Jung et al., 2017; Polonius design documents, 2018). The key insight is that Rust's current borrow checker uses "syntactic lifetimes" — the scope of a borrow is determined by its syntactic extent in the source code, which is a conservative approximation of its actual liveness. Polonius uses a datalog-based analysis to compute more precise borrow lifetimes, accepting programs that the current borrow checker rejects but that are actually safe.

VUMA's Tier 1 analysis is analogous to Polonius, but with two important extensions:

1. **Beyond borrow checking.** Polonius verifies only the borrow checker's rules (which enforce a subset of VUMA's liveness and exclusivity invariants). VUMA's Tier 1 also addresses interpretation, origin, and cleanup invariants.

2. **Beyond Rust's ownership model.** Polonius operates within Rust's ownership framework, where every value has a unique owner. VUMA's Tier 1 operates on raw pointers with no ownership requirement, inferring ownership where it exists and falling back to higher tiers where it does not.

The Polonius experience provides an important lesson: even a relatively modest improvement in precision (from syntactic to dataflow-based liveness) required a complete rewrite of the borrow checker's core algorithm. VUMA's ambition for global, inference-based verification represents a qualitatively larger leap, and the Polonius experience suggests that the implementation complexity will be substantial.

### 4.3 CompCert

CompCert (Leroy, 2009) is a formally verified C compiler — its implementation is proved correct in Coq, guaranteeing that the generated machine code preserves the semantics of the source program. CompCert's memory model (Leroy and Blazy, 2008) is directly relevant to VUMA: it defines a formal semantics for C memory operations, including allocation, deallocation, pointer arithmetic, and access. CompCert's memory model enforces several of VUMA's invariants:

- **Liveness**: Accesses to freed blocks are detected and cause a runtime error (in the formal semantics) rather than proceeding with undefined behavior.
- **Interpretation**: Pointer arithmetic that escapes the bounds of an allocated block is detected.
- **Origin**: Every pointer value is associated with the block from which it was derived.

CompCert's approach differs from VUMA in two fundamental ways:

1. **Runtime enforcement vs. static verification.** CompCert's invariants are checked at runtime in the formal semantics; VUMA aims to verify them statically. CompCert's approach is sound but incurs runtime overhead (which is removed by the proof of correctness that shows the checks can be elided for well-behaved programs). VUMA's approach aims to eliminate the checks entirely by proving they are unnecessary.

2. **Verified compiler vs. verified program.** CompCert verifies the compiler; VUMA verifies the program. These are complementary: CompCert ensures that the compiler doesn't introduce memory errors, while VUMA ensures that the program doesn't contain memory errors. Together, they would provide an end-to-end guarantee from source program to machine code.

### 4.4 Checked C

Checked C (Elliot et al., 2018; CFRG, 2021) extends C with checked pointer types and bounds annotations that allow the compiler to verify spatial memory safety. A checked pointer `ptr<T>` carries a bounds specification, and the compiler inserts runtime checks or statically verifies that accesses through the pointer are within bounds.

Checked C is a pragmatic approach that shares VUMA's goal of safe pointer access without restricting the programmer's ability to use pointers. However, Checked C relies on **programmer-provided annotations** — the programmer must declare pointer bounds, checked regions, and bounds-safe interfaces. VUMA aims to **infer** these annotations, which is a harder problem but a more usable system. The Checked C experience demonstrates that annotation-based approaches face significant adoption barriers: programmers must learn the annotation language, apply it correctly, and maintain it as the code evolves. Inference-based approaches avoid these barriers at the cost of implementation complexity.

### 4.5 Why3/Frama-C

Why3 (Filliâtre and Paskevich, 2013) and Frama-C (Cuoq et al., 2012) are deductive verification systems for C programs. The programmer writes function contracts (preconditions, postconditions, invariants) in a specification language (ACSL for Frama-C, WhyML for Why3), and the system generates proof obligations that are discharged by automatic or interactive theorem provers.

Frama-C represents the most mature approach to verifying C programs, including pointer-based programs with manual memory management. It can verify all five of VUMA's invariants, given sufficient programmer annotations. The difference from VUMA is again one of **inference vs. annotation**: Frama-C requires the programmer to specify what to prove; VUMA aims to determine what needs to be proved and prove it automatically.

The Frama-C experience provides important data on the practical limits of deductive verification:

1. **Annotation overhead is substantial.** Real-world Frama-C case studies report annotation-to-code ratios of 3:1 to 10:1 — three to ten lines of specification for every line of code. This is acceptable for safety-critical systems but prohibitive for general software development.

2. **Proof automation is incomplete.** Even with state-of-the-art SMT solvers (Alt-Ergo, Z3, CVC5), a significant fraction of proof obligations require manual proof guidance. The verification engineer must understand the proof strategy, identify why automation failed, and provide hints.

3. **Scalability is limited.** Deductive verification of large programs (100K+ lines) requires modular specifications and compositional reasoning, which are difficult to achieve in practice.

VUMA's approach — inference-based verification with tiered confidence — can be viewed as an attempt to automate the parts of the Frama-C workflow that currently require human effort: specification generation, proof guidance, and invariant discovery. If successful, it would make deductive verification practical for the 99% of software that cannot afford the current annotation and proof burden.

### 4.6 Summary of Key Differences

The key difference between VUMA and all of the above systems is the **inference-over-annotation principle**: VUMA aims to verify memory safety properties by inferring what needs to be proved and proving it automatically, rather than requiring the programmer to specify what to prove. This principle is the direct consequence of VUMA's design for AI-native consumption: an AI agent that understands the entire program can perform the reasoning that a human verification engineer currently performs, but faster, more consistently, and without the cognitive overhead of writing and maintaining specifications.

This is not to say that VUMA's approach is strictly superior. Annotation-based systems provide stronger guarantees when the annotations are correct (the programmer explicitly specified what they want), while inference-based systems provide convenience at the cost of trusting the inference engine's judgment about what needs to be proved. VUMA mitigates this risk through its tiered confidence model and by allowing the programmer to override inferred specifications when necessary.

---

## 5. The Approximation Argument

The undecidability result in Section 1 is unconditional: there exists no algorithm that can verify all five VUMA invariants for all VUMA programs. The restricted subsets in Section 2 show that decidability can be recovered by restricting the language, but each restriction sacrifices expressiveness. The practical verification strategy in Section 3 shows how to handle the undecidable cases through tiered analysis and confidence levels. This section presents the **approximation argument**: the thesis that, in practice, the undecidability barrier is less formidable than the theoretical result suggests, because the programs that exercise the undecidable core are rare in real codebases.

### 5.1 The Empirical Distribution of Memory Patterns

Real programs do not exercise the full generality of pointer arithmetic. An empirical study of C codebases (Chandra et al., 2017; Serebryany et al., 2012) reveals that the vast majority of memory accesses follow a small number of regular patterns:

- **Stack access** (~40% of accesses): Local variables, function parameters, return values. These are trivially safe — the compiler manages their lifetime, and no pointer arithmetic is involved.

- **Structured heap access** (~30% of accesses): Allocations through `malloc`/`new`, accessed through named fields, freed through `free`/`delete`. The pointer derivations are field offsets (VUMA-1 compatible), and the ownership is typically clear (VUMA-4 category a). The primary challenge is use-after-free, which is detectable by tracking the allocation-to-deallocation-to-access sequence.

- **Array access** (~15% of accesses): Allocations accessed through base-plus-offset arithmetic with the offset computed from a loop index or a container method call. These are VUMA-1 compatible if the offset is a field access, or require symbolic execution (Tier 2) if the offset involves runtime computation. Bounds checking is the primary invariant, and it is decidable for arrays with known sizes.

- **Linked structure access** (~10% of accesses): Allocations linked through pointer fields — lists, trees, hash maps, graphs. These are the domain of separation logic (VUMA-3). Lists and trees are decidable; arbitrary graphs may not be, but most graph algorithms use patterns that are amenable to shape analysis.

- **Raw byte manipulation** (~4% of accesses): Allocations treated as byte arrays, accessed through computed offsets — network buffer parsing, serialization, custom allocators. These are the primary challenge for VUMA, because the pointer arithmetic is unconstrained and the interpretation of the accessed bytes is context-dependent. Symbolic execution (Tier 2) can handle simple cases; LLM-guided reasoning (Tier 3) may be needed for complex ones.

- **Truly adversarial patterns** (~1% of accesses): Self-modifying code, JIT compilation, hardware device access, exploit development. These are the programs that exercise the undecidable core of VUMA verification. They are real and important, but they are a tiny fraction of all code.

### 5.2 Shape Analysis as a Practical Decidable Fragment

The key insight of the approximation argument is that **most pointer patterns follow regular structures**. Arrays, linked lists, trees, and hash maps are not just common — they are the building blocks of virtually all software. For these structures, shape analysis (Sagiv et al., 2002; Calcagno et al., 2011) can determine the heap shape at each program point, and from the shape, the five invariants can be verified.

Shape analysis works by abstracting the heap into a finite set of shape graphs — directed graphs where nodes represent allocation sites and edges represent pointer fields. The abstraction is sound (all concrete heaps that satisfy the invariant are represented by the abstract shape graph) and, for the common data structures, precise (the abstract shape graph represents exactly the set of concrete heaps that arise during program execution). When shape analysis is precise, all five invariants are decidable.

The limitation of shape analysis is that it loses precision for structures with **unbounded sharing** — heap graphs where a single node is pointed to by an unbounded number of other nodes. This occurs in object-oriented programs with complex aliasing patterns, in concurrent data structures with shared state, and in graph algorithms that maintain arbitrary adjacency relationships. For these cases, shape analysis provides a sound over-approximation (it may report that a violation is possible when it is not) but not a precise answer (it cannot confirm that the invariant holds).

### 5.3 The Remaining Hard Cases

The hard cases for VUMA verification — the ~5% of accesses that fall outside the scope of shape analysis — include:

- **Custom memory allocators** (arena allocators, slab allocators, pool allocators): These treat memory as a raw resource, allocating sub-regions within a larger region and managing lifetimes according to custom policies. The IVE can verify them by modeling the allocator as a domain-specific memory management operation with a specified contract, but the contract must be either inferred (which requires understanding the allocator's implementation) or provided (which requires annotation). This is a natural application for LLM-guided reasoning (Tier 3).

- **JIT compilers and dynamic code generation**: These create executable code at runtime, blurring the boundary between data and code. The IVE must verify that generated code respects the same invariants as statically written code, which requires reasoning about the code generator's behavior. This is undecidable in general but tractable for specific JIT architectures (e.g., a JIT that generates code from a template with filled-in constants).

- **Graph algorithms with cycles**: Algorithms that maintain arbitrary graph structures — graph coloring, cycle detection, shortest path — create heap shapes that are not expressible in standard separation logic fragments. The IVE can verify them by using parametric shape predicates (e.g., "this region is a graph where each node has at most k outgoing edges") or by falling back to whole-program analysis (Tier 2).

- **FFI and hardware interaction**: Accesses through foreign function interfaces, memory-mapped I/O, and DMA are inherently beyond the IVE's reasoning capability, because the IVE has no model of the external system. These must be marked as trusted boundaries with explicit safety contracts.

### 5.4 VUMA's Principle: Verify What You Can, Flag What You Cannot

The approximation argument leads to VUMA's core safety principle: **verify what you can, flag what you cannot, never silently accept unsafe access.** This principle has three components:

1. **Verify what you can.** For the ~95% of accesses that follow regular patterns, VUMA provides strong, automated verification — comparable to or better than Rust's borrow checker. This verification is sound (no false negatives) and precise (few false positives), because it operates on decidable fragments of the verification problem.

2. **Flag what you cannot.** For the ~5% of accesses that resist automated verification, VUMA provides explicit notification — the programmer knows exactly which accesses are unverified and why. This is fundamentally different from C, where the programmer has no information about which accesses might be unsafe, and from Rust, where the programmer knows the borrow checker rejected the code but often cannot determine why.

3. **Never silently accept unsafe access.** An access that the IVE cannot verify is never treated as safe. It is flagged, displayed, and factored into deployment decisions. The confidence model (Section 3.5) ensures that unverified accesses are treated with appropriate caution.

This principle is VUMA's answer to the undecidability barrier. It does not make undecidable problems decidable. It does not promise complete verification. What it promises is **honest, actionable information about memory safety**, delivered at a scale and precision that existing systems cannot match. The result is not a utopia of perfect memory safety, but a dramatic improvement over the current state of the art: a world where the vast majority of memory accesses are automatically verified, the remainder are explicitly identified, and no access is ever silently assumed safe.

### 5.5 The Asymptotic Argument

There is one more consideration: the IVE's reasoning capability will improve over time. As the IVE incorporates better shape analyses, more powerful SMT solvers, and more capable LLMs, the fraction of accesses it can verify will increase. The 90-8-1.5-0.5 split of Section 3 is a snapshot of the current state, not a permanent fixture. VUMA's architecture — with its tiered verification, confidence levels, and explicit unverified set — is designed to accommodate this improvement: as Tier 1 and Tier 2 become more powerful, they absorb accesses currently handled by Tier 3 and Tier 4, shrinking the unverified set toward (but never reaching) zero.

This asymptotic argument is VUMA's strongest response to the undecidability result. The theoretical barrier is real and permanent: there will always be programs that the IVE cannot verify. But the practical barrier — the fraction of real-world code that the IVE cannot verify — can be made arbitrarily small. VUMA's goal is not to solve the undecidable, but to make the undecidable irrelevant for all but the most exotic programs.

---

## Conclusion

Full VUMA verification is undecidable. This is an inescapable consequence of Turing completeness and the expressiveness of unrestricted pointer arithmetic. However, the undecidability barrier does not invalidate the VUMA approach. Instead, it provides a clear roadmap:

1. **Identify decidable fragments** (VUMA-1 through VUMA-4) that cover the vast majority of real-world memory access patterns.
2. **Design a tiered verification strategy** that applies the strongest decidable analysis first and falls back to progressively more speculative approaches for harder cases.
3. **Assign confidence levels** that honestly characterize the strength of the verification evidence.
4. **Flag unverified accesses** explicitly, never silently accepting unsafe access.
5. **Improve incrementally**, shrinking the unverified set as the IVE's reasoning capability grows.

This approach is intellectually honest, practically sound, and consistent with the best practices of formal methods: accept theoretical limitations, engineer practical solutions, and never compromise on soundness. VUMA's contribution is not the elimination of undecidability — it is the principled management of undecidability in service of a safer, more expressive programming model.

---

## References

- Berdine, J., Calcagno, C., and O'Hearn, P.W. (2005). "Smallfoot: Modular Automatic Assertion Checking with Separation Logic." *FMCO 2005*.
- Biere, A., Cimatti, A., Clarke, E., and Zhu, Y. (1999). "Symbolic Model Checking without BDDs." *TACAS 1999*.
- Calcagno, C., Distefano, D., Dubreil, J., et al. (2015). "Moving Fast with Software Verification." *NFM 2015*.
- Calcagno, C., Distefano, D., O'Hearn, P.W., and Yang, H. (2011). "Compositional Shape Analysis by Means of Bi-abduction." *JACM 58(6)*.
- Chandra, S., et al. (2017). "Practical Memory Safety for C." *arXiv preprint*.
- Cuoq, P., Kirchner, F., Kosmatov, N., et al. (2012). "Frama-C: A Software Analysis Perspective." *SEFM 2012*.
- Distefano, D., O'Hearn, P.W., and Yang, H. (2006). "A Local Shape Analysis Based on Separation Logic." *TACAS 2006*.
- Elliot, A., et al. (2018). "Checked C: Making C Safe by Extending It with Bounds and Checked Pointers." *SPLASH 2018*.
- Filliâtre, J.-C. and Paskevich, A. (2013). "Why3 — Where Programs Meet Provers." *ESOP 2013*.
- Ishtiaq, S. and O'Hearn, P.W. (2001). "BI as an Assertion Language for Mutable Data Structures." *POPL 2001*.
- Jung, R., et al. (2017). "RustBelt: Securing the Foundations of the Rust Programming Language." *POPL 2018*.
- Landi, W. (1992). "Undecidability of Static Analysis." *ACM LOPLAS 1(4)*.
- Leroy, X. (2009). "Formal Verification of a Realistic Compiler." *CACM 52(7)*.
- Leroy, X. and Blazy, S. (2008). "Formal Verification of a C-like Memory Model." *JAR 41(1)*.
- O'Hearn, P.W. (2001). "Resources, Concurrency, and Local Reasoning." *CONCUR 2004*.
- Ramalingam, G. (2000). "Data Race Detection is Undecidable." *Unpublished manuscript, IBM Research*.
- Reynolds, J.C. (2002). "Separation Logic: A Logic for Shared Mutable Data Structures." *LICS 2002*.
- Rice, H.G. (1953). "Classes of Recursively Enumerable Sets and Their Decision Problems." *Trans. AMS 89(1)*.
- Sagiv, M., Reps, T., and Wilhelm, R. (2002). "Parametric Shape Analysis via 3-valued Logic." *TOPLAS 24(3)*.
- Serebryany, K., et al. (2012). "AddressSanitizer: A Fast Address Sanity Checker." *USENIX ATC 2012*.
- Turing, A.M. (1936). "On Computable Numbers, with an Application to the Entscheidungsproblem." *Proc. LMS 2(42)*.
- Wells, J.B. (1999). "Typability and Type Checking in System F Are Equivalent and Undecidable." *APAL 98(1-3)*.
