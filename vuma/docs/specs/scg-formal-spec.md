# Formal Mathematical Specification: Semantic Computation Graph (SCG)

**Project:** VUMA — Verified-Unsafe Memory Access  
**Document:** W1-01 — SCG Formal Model  
**Author:** Agent W1-01  
**Date:** 2026-03-04  
**Status:** Specification — Draft 1  

---

## Preamble

This document provides a rigorous mathematical specification for the Semantic Computation Graph (SCG), the primary intermediate representation of programs in the VUMA framework. The SCG replaces traditional source code as the canonical representation of a program: it is a directed, annotated graph whose nodes denote computational operations and whose edges denote data flow, control flow, derivation chains, and behavioral annotations. The formalism presented here is designed to be machine-reasonable by the Inference and Verification Engine (IVE), to support formally defined composition operators, and to admit a bisimulation-based equivalence relation that yields a unique canonical form.

We adopt the following notational conventions throughout:

- **Sets** are denoted by uppercase italic letters (e.g., $N$, $E$, $V$).  
- **Tuples** are written in angle brackets (e.g., $\langle n, \ell, m \rangle$).  
- **Functions** are lowercase Greek or italic (e.g., $\sigma$, $\text{src}$, $\text{tgt}$).  
- **Type constructors** are capitalized (e.g., $\text{ComputationNode}$, $\text{DataFlow}$).  
- **Predicates** are sans-serif (e.g., $\mathsf{wellFormed}$, $\mathsf{acyclic}$).  
- **Quantifiers** follow standard mathematical usage: $\forall$ (for all), $\exists$ (there exists), $\exists!$ (there exists exactly one).  

---

## 1. SCG Node Types

### 1.1 Definition: Node Universe

Let $\mathcal{N}$ denote the universe of all possible SCG nodes. Every node $n \in \mathcal{N}$ is an element of a disjoint-sum algebraic data type (ADT) comprising eight constructors. We define:

$$
\mathcal{N} = \text{ComputationNode}(\mathcal{F}, \mathcal{V}^k, \mathcal{BD}) \;+\; \text{AllocationNode}(\mathcal{R}, \mathcal{BD}) \;+\; \text{DeallocationNode}(\mathcal{R}^*, \mathcal{BD}) \\
+\; \text{AccessNode}(\mathcal{A}, \mathcal{M}, \mathcal{BD}) \;+\; \text{CastNode}(\mathcal{BD}_{\text{src}}, \mathcal{BD}_{\text{tgt}}, \mathcal{BD}) \\
+\; \text{EffectNode}(\mathcal{E}, \mathcal{BD}) \;+\; \text{ControlNode}(\mathcal{C}, \mathcal{BD}) \;+\; \text{PhantomNode}(\mathcal{BD})
$$

where:

- $\mathcal{F}$ is the set of function identifiers (including primitive operations).  
- $\mathcal{V}$ is the set of value identifiers. $\mathcal{V}^k$ denotes a $k$-ary sequence of value arguments.  
- $\mathcal{BD} = \text{RepD} \times \text{CapD} \times \text{RelD}$ is the Behavioral Descriptor triple as defined in the VUMA proposal (Section 3.5). Every node carries a BD annotation that specifies the representation, capability, and relational properties of the value it produces.  
- $\mathcal{R}$ is the set of region identifiers (contiguous address ranges in the VUMA memory model).  
- $\mathcal{R}^*$ is a reference to the region being deallocated.  
- $\mathcal{A}$ is the set of address values (numeric locations in the address space).  
- $\mathcal{M} = \{\text{Read}, \text{Write}\}$ is the access mode.  
- $\mathcal{E}$ is the set of side-effect descriptors (I/O, network, logging, etc.).  
- $\mathcal{C} = \{\text{Branch}(v), \text{Merge}(k), \text{Loop}(\text{body}, \text{guard})\}$ is the set of control constructs.  
- $\mathcal{BD}_{\text{src}}$ and $\mathcal{BD}_{\text{tgt}}$ are the source and target behavioral descriptors for a reinterpretation cast.  

### 1.2 Constructor Semantics

Each constructor has precise semantics governing its role in the SCG:

**ComputationNode($f$, $[v_1, \ldots, v_k]$, $bd$):** Represents the application of function $f$ to arguments $v_1, \ldots, v_k$, producing a value with behavioral descriptor $bd$. The function $f$ may be a user-defined function, a primitive operation (arithmetic, comparison, etc.), or a closure. The arity $k$ must match the function signature's expected parameter count. The output BD $bd$ is constrained by the function's declared effect: if $f$ is pure, then $bd.\text{CapD} \subseteq \{\text{read}, \text{compare}, \text{hash}, \text{serialize}\}$; if $f$ is impure, the CapD may include side-effect capabilities. ComputationNode is the workhorse of the SCG — it subsumes what traditional ASTs represent as function calls, operator applications, and literal constructions. Importantly, a ComputationNode does not merely record *that* a function was applied; it records *what behavioral properties* the result carries, enabling the IVE to reason about the result without re-analyzing the function body.

**AllocationNode($r$, $bd$):** Represents the allocation of a memory region $r$ with initial behavioral descriptor $bd$. The region $r$ is a fresh identifier drawn from $\mathcal{R}$. The RepD component of $bd$ specifies the layout (size, alignment) of the allocated region. The CapD specifies the initial capabilities (typically $\{\text{read}, \text{write}\}$). The RelD records the allocation context — for instance, that $r$ was allocated within a particular SCG region (scope, phase, security boundary). AllocationNode is the VUMA analog of `malloc`, `new`, or stack allocation, but it is not annotated with a traditional type — only with a BD. The IVE tracks the liveness of $r$ from this node forward, until a matching DeallocationNode or the end of the owning scope.

**DeallocationNode($r^*$, $bd$):** Represents the deallocation of region $r$. The BD $bd$ describes the post-deallocation state — notably, the CapD is $\emptyset$ (no operations are valid on freed memory) and the RelD records that $r$ is now dead. The IVE must verify that no AccessNode targeting $r$ is reachable after this node in any execution path. The well-formedness condition (Section 5) enforces that every DeallocationNode has a corresponding AllocationNode and that no dangling references exist.

**AccessNode($a$, $m$, $bd$):** Represents a read or write of address $a$ in mode $m$. The address $a$ must be derivable (via Derivation edges) from some AllocationNode. The BD $bd$ specifies the expected interpretation: the RepD describes how the bytes at $a$ are to be interpreted, the CapD must include the access mode (Read requires $\text{read} \in \text{CapD}$; Write requires $\text{write} \in \text{CapD}$), and the RelD records any temporal constraints on when this access is valid. AccessNode is the central node type for VUMA verification — the IVE must prove that every AccessNode satisfies the liveness, exclusivity, interpretation, and origin invariants (Proposal Section 3.6.2).

**CastNode($bd_{\text{src}}$, $bd_{\text{tgt}}$, $bd$):** Represents a reinterpretation of data from source descriptor $bd_{\text{src}}$ to target descriptor $bd_{\text{tgt}}$. The output BD is $bd_{\text{tgt}}$. CastNode does not copy or transform data — it changes the *perspective* through which the data is viewed, consistent with the BD philosophy that the same bytes can have multiple simultaneous interpretations. The IVE must verify that the reinterpretation is valid: the RepD of $bd_{\text{tgt}}$ must be a valid interpretation of the memory described by $bd_{\text{src}}$'s RepD, and the CapD of $bd_{\text{tgt}}$ must not grant capabilities that $bd_{\text{src}}$ withholds (capability narrowing is safe; widening is not, unless provably justified by context).

**EffectNode($e$, $bd$):** Represents a side effect $e$ (I/O, network transmission, logging, mutation of external state). The BD $bd$ describes the observable behavioral footprint of the effect. EffectNodes are distinguished from ComputationNodes by their mandatory side-effect descriptor and by the fact that their execution ordering is constrained by ControlFlow edges. The IVE tracks EffectNodes to verify temporal properties: every I/O operation occurs in the correct phase, every network send has a corresponding receive, and every lock acquisition has a matching release.

**ControlNode($c$, $bd$):** Represents control flow structure. The three variants are: (1) $\text{Branch}(v)$: conditional divergence based on value $v$, producing two successor subgraphs (true-branch and false-branch); (2) $\text{Merge}(k)$: convergence of $k$ control flow paths, with a value selected from among the $k$ incoming paths; (3) $\text{Loop}(\text{body}, \text{guard})$: iterative execution of subgraph $\text{body}$ while $\text{guard}$ evaluates to true. The BD of a ControlNode is derived from the BDs of its successors: a Branch node's BD is the union of its branches' BDs, a Merge node's BD is the join (least upper bound) of its inputs' BDs, and a Loop node's BD is the fixed point of the body's BD transformer.

**PhantomNode($bd$):** Represents a type-level computation that has no runtime counterpart. PhantomNodes exist solely to carry BD information through the graph for the purpose of IVE reasoning. They are erased during execution but are essential for verification: a PhantomNode may assert that a certain invariant holds, that a security level is maintained, or that a temporal constraint is satisfied. They are analogous to phantom types in Haskell or Rust, but generalized to carry arbitrary BD annotations rather than just type-level markers. The IVE uses PhantomNodes to propagate constraints that would otherwise be lost in a purely operational representation.

### 1.3 Node Identity and Uniqueness

Every node $n \in \mathcal{N}$ has a globally unique identifier $\text{id}(n) \in \mathbb{N}^+$ assigned by a monotonic counter. No two nodes in any SCG share an identifier. Furthermore, we define a projection function:

$$
\text{kind} : \mathcal{N} \to \{\text{Comp}, \text{Alloc}, \text{Dealloc}, \text{Access}, \text{Cast}, \text{Effect}, \text{Control}, \text{Phantom}\}
$$

that maps each node to its constructor tag. This enables pattern-matching dispatch in the IVE and in composition operators.

---

## 2. Edge Semantics

### 2.1 Definition: Edge Universe

Let $\mathcal{E}$ denote the universe of all possible SCG edges. An edge is a labeled, directed relation between two nodes:

$$
\mathcal{E} = \{ \langle n_s, \ell, n_t \rangle \mid n_s, n_t \in \mathcal{N},\; \ell \in \mathcal{L} \}
$$

where $\mathcal{L} = \{\text{DataFlow}, \text{ControlFlow}, \text{Derivation}, \text{Annotation}\}$ is the set of edge labels. We define the source and target projection functions:

$$
\text{src}(\langle n_s, \ell, n_t \rangle) = n_s, \qquad \text{tgt}(\langle n_s, \ell, n_t \rangle) = n_t, \qquad \text{label}(\langle n_s, \ell, n_t \rangle) = \ell
$$

### 2.2 Edge Label Semantics

Each edge label carries distinct semantic meaning, and each is subject to specific structural constraints governing where it may appear in a well-formed SCG:

**DataFlow ($n_s \xrightarrow{\text{DF}} n_t$):** The value produced by node $n_s$ is consumed as an input by node $n_t$. This is the primary dependency relation in the SCG. A DataFlow edge asserts that $n_t$ cannot execute until $n_s$ has produced its value, and that the value flows without modification (unless an intervening CastNode changes the BD perspective). The BD of the value on this edge is exactly $n_s$'s output BD. Formally, if $n_s \xrightarrow{\text{DF}} n_t$ and $n_s = \text{ComputationNode}(f, [v_1, \ldots, v_k], bd_s)$, then the value consumed by $n_t$ at this input port has behavioral descriptor $bd_s$. DataFlow edges carry an additional port index $\pi \in \mathbb{N}$ identifying which input of $n_t$ this value satisfies, since a node may have multiple inputs. We write $n_s \xrightarrow{\text{DF}(\pi)} n_t$ when the port is significant. The DataFlow subgraph must be acyclic (see Section 5, Well-formedness Condition 2).

**ControlFlow ($n_s \xrightarrow{\text{CF}} n_t$):** Node $n_t$ may execute only after $n_s$ has executed, and the execution of $n_s$ causally enables $n_t$. Unlike DataFlow edges, ControlFlow edges do not carry values — they carry execution ordering constraints. ControlFlow edges are essential for ordering EffectNodes (which may not have data dependencies but must be sequenced), for connecting ControlNodes to their branch targets, and for expressing happens-before relationships in concurrent subgraphs. A ControlFlow edge is valid only if: (1) $n_s$ is a ControlNode, EffectNode, or an entry node of the SCG, and (2) $n_t$ is reachable from $n_s$ in the control-flow sense. ControlFlow edges *may* form cycles (representing loops), but these cycles must originate from ControlNode(Loop) nodes and be well-structured (see Section 5).

**Derivation ($n_s \xrightarrow{\text{Deriv}} n_t$):** The address value produced by $n_t$ is derived from the region allocated by $n_s$. This edge exists if and only if $n_s = \text{AllocationNode}(r, \_)$ and $n_t$ produces a value that is an address within region $r$. Derivation edges form chains: if $n_0 \xrightarrow{\text{Deriv}} n_1 \xrightarrow{\text{Deriv}} n_2$, then the address produced by $n_2$ is derived from the region allocated by $n_0$ via the intermediate address produced by $n_1$. These chains are critical for VUMA's origin invariant (Proposal Section 3.6.2): every AccessNode must have a Derivation path from some AllocationNode. Derivation edges are also how the IVE tracks pointer arithmetic: if a ComputationNode computes `base + offset` where `base` is derived from region $r$, the result is also derived from $r$, and a Derivation edge records this fact. The Derivation subgraph is a forest rooted at AllocationNodes.

**Annotation ($n_s \xrightarrow{\text{Ann}} n_t$):** The BD produced or carried by $n_s$ is attached as a behavioral annotation to $n_t$. Annotation edges are the mechanism by which the IVE propagates and attaches behavioral constraints to nodes that do not intrinsically produce them. For example, a PhantomNode carrying a security-level assertion may be connected via an Annotation edge to an AccessNode, indicating that the access is valid only under the asserted security condition. Annotation edges do not affect execution order or data flow — they are purely informational for the IVE. However, they are subject to a consistency constraint: the BD carried by $n_s$ must be *compatible* with $n_t$'s own BD, in the sense that the CapD of the annotation must be a subset of the CapD of $n_t$ (annotations can restrict, not expand, capabilities), and the RelD of the annotation must be consistent with the RelD of $n_t$ (no contradictions in relational assertions).

### 2.3 Edge Multiplicity and Port Assignment

We define the multiplicity constraints for each edge type:

| Edge Type | Source Multiplicity | Target Multiplicity | Port-Indexed |
|-----------|--------------------|--------------------|--------------|
| DataFlow  | $n_s$ has exactly one outgoing DF per value produced | $n_t$ has exactly one incoming DF per input port | Yes |
| ControlFlow | $n_s$ has $\geq 1$ outgoing CF | $n_t$ has $\geq 1$ incoming CF | No |
| Derivation | $n_s$ (AllocationNode) has $\geq 0$ outgoing Deriv | $n_t$ has exactly one incoming Deriv | No |
| Annotation | $n_s$ has $\geq 0$ outgoing Ann | $n_t$ has $\geq 0$ incoming Ann | No |

Formally, for a given SCG $G = (N, E)$, we define:

$$
\text{in}_{\ell}(n) = \{ e \in E \mid \text{tgt}(e) = n \wedge \text{label}(e) = \ell \}
$$
$$
\text{out}_{\ell}(n) = \{ e \in E \mid \text{src}(e) = n \wedge \text{label}(e) = \ell \}
$$

The DataFlow port assignment is a function $\text{port} : \{ e \in E \mid \text{label}(e) = \text{DataFlow} \} \to \mathbb{N}$ that is injective on the incoming DataFlow edges of any node: for any $n \in N$ and distinct $e_1, e_2 \in \text{in}_{\text{DF}}(n)$, $\text{port}(e_1) \neq \text{port}(e_2)$.

### 2.4 Edge BD Propagation

Every DataFlow edge implicitly carries the BD of its source node's output. We define the edge BD propagation function:

$$
\beta : E_{\text{DF}} \to \mathcal{BD}, \qquad \beta(n_s \xrightarrow{\text{DF}(\pi)} n_t) = \text{outBD}(n_s)
$$

where $\text{outBD} : \mathcal{N} \to \mathcal{BD}$ extracts the output behavioral descriptor of a node (the $bd$ field in each constructor). The IVE uses $\beta$ to verify type consistency across the graph (Well-formedness Condition 4).

---

## 3. Composition Operators

### 3.1 Definition: SCG as a Formal Object

An SCG is a tuple $G = (N, E, \iota, o)$ where:

- $N \subset \mathcal{N}$ is a finite set of nodes.  
- $E \subset \mathcal{E}$ is a finite set of edges with $\text{src}(e), \text{tgt}(e) \in N$ for all $e \in E$.  
- $\iota : \mathbb{N} \rightharpoonup N$ is a partial function mapping input ports to nodes (the interface by which this SCG receives values from its environment).  
- $o : \mathbb{N} \rightharpoonup N$ is a partial function mapping output ports to nodes (the interface by which this SCG provides values to its environment).  

The input/output port structure makes an SCG a *composable* unit — it exposes typed, indexed ports through which data flows in and out, analogous to a function signature but at the graph level.

### 3.2 Sequential Composition ($G_1 \mathbin{;} G_2$)

**Definition.** Given SCGs $G_1 = (N_1, E_1, \iota_1, o_1)$ and $G_2 = (N_2, E_2, \iota_2, o_2)$, the sequential composition $G_1 \mathbin{;} G_2$ is defined when $|\text{dom}(o_1)| = |\text{dom}(\iota_2)|$ (the number of outputs of $G_1$ matches the number of inputs of $G_2$) and the BDs are compatible at each connection point. Specifically, for each port index $k \in \text{dom}(o_1) \cap \text{dom}(\iota_2)$:

$$
\text{outBD}(o_1(k)) \sqsubseteq_{\text{BD}} \text{expectedBD}(\iota_2(k))
$$

where $\sqsubseteq_{\text{BD}}$ is the BD compatibility relation: $(r_1, c_1, d_1) \sqsubseteq_{\text{BD}} (r_2, c_2, d_2)$ iff $r_1$ and $r_2$ are layout-compatible (same size and alignment), $c_1 \supseteq c_2$ (the producer provides at least the capabilities the consumer requires), and $d_1$ and $d_2$ are relationally consistent (no contradictions in temporal or security constraints).

When defined, $G_1 \mathbin{;} G_2 = (N', E', \iota', o')$ where:

$$
N' = N_1 \uplus N_2 \quad \text{(disjoint union)}
$$
$$
E' = E_1 \uplus E_2 \uplus \{ o_1(k) \xrightarrow{\text{DF}(k)} \iota_2(k) \mid k \in \text{dom}(o_1) \cap \text{dom}(\iota_2) \}
$$
$$
\iota'(k) = \iota_1(k) \quad \text{(inputs come from } G_1\text{)}
$$
$$
o'(k) = o_2(k) \quad \text{(outputs come from } G_2\text{)}
$$

**Properties.** Sequential composition is:  
- **Associative:** $(G_1 \mathbin{;} G_2) \mathbin{;} G_3 = G_1 \mathbin{;} (G_2 \mathbin{;} G_3)$ when both sides are defined. This follows from the associativity of disjoint union and the transitivity of BD compatibility.  
- **Non-commutative:** $G_1 \mathbin{;} G_2 \neq G_2 \mathbin{;} G_1$ in general, because the data flow direction is fixed.  
- **Identity element:** The identity SCG $\mathsf{id}_n = (\emptyset, \emptyset, \iota, o)$ where $\iota(k) = o(k)$ for $k \in \{1, \ldots, n\}$ (a "pass-through" graph with no nodes) satisfies $G \mathbin{;} \mathsf{id}_n = G = \mathsf{id}_m \mathbin{;} G$ for appropriate $m, n$.

The sequential composition operator is the SCG analog of function composition or pipeline wiring. It is the fundamental building block for constructing programs: every straight-line code sequence compiles to a chain of sequential compositions.

### 3.3 Parallel Composition ($G_1 \parallel G_2$)

**Definition.** Given SCGs $G_1 = (N_1, E_1, \iota_1, o_1)$ and $G_2 = (N_2, E_2, \iota_2, o_2)$, the parallel composition $G_1 \parallel G_2$ is always defined (no BD compatibility check is needed at the composition boundary, since the subgraphs do not directly connect). The result is:

$$
N' = N_1 \uplus N_2
$$
$$
E' = E_1 \uplus E_2
$$
$$
\iota'(k) = \begin{cases} \iota_1(k) & \text{if } k \in \text{dom}(\iota_1) \\ \iota_2(k - |\text{dom}(\iota_1)|) & \text{if } k \in \{|\text{dom}(\iota_1)| + 1, \ldots, |\text{dom}(\iota_1)| + |\text{dom}(\iota_2)|\} \end{cases}
$$
$$
o'(k) \text{ defined analogously, concatenating outputs}
$$

**Properties.** Parallel composition is:  
- **Commutative:** $G_1 \parallel G_2 \cong G_2 \parallel G_1$ (isomorphic up to port renumbering).  
- **Associative:** $(G_1 \parallel G_2) \parallel G_3 \cong G_1 \parallel (G_2 \parallel G_3)$.  
- **Distributes over sequential composition** under certain conditions: $(G_1 \parallel G_2) \mathbin{;} (G_3 \parallel G_4) = (G_1 \mathbin{;} G_3) \parallel (G_2 \mathbin{;} G_4)$ when the port arities and BD compatibilities align.

Parallel composition is the natural representation of concurrent computation. In the VUMA framework, parallelism is the default — sequential composition is a special case where data dependencies force ordering. The IVE verifies that parallel subgraphs do not have conflicting memory accesses (exclusivity invariant), which is the VUMA replacement for Rust's borrow checker.

### 3.4 Conditional Composition ($\text{if } G_c \text{ then } G_t \text{ else } G_f$)

**Definition.** Given a condition SCG $G_c$, a true-branch SCG $G_t$, and a false-branch SCG $G_f$, the conditional composition constructs an SCG with a Branch ControlNode:

$$
G_{\text{cond}} = (N', E', \iota', o')
$$

where:

$$
n_{\text{branch}} = \text{ControlNode}(\text{Branch}(v_c), bd_{\text{branch}})
$$
$$
N' = \{n_{\text{branch}}\} \uplus N_c \uplus N_t \uplus N_f \uplus \{n_{\text{merge}}\}
$$
$$
n_{\text{merge}} = \text{ControlNode}(\text{Merge}(2), bd_{\text{merge}})
$$
$$
E' = E_c \uplus E_t \uplus E_f \uplus \{ o_c(1) \xrightarrow{\text{DF}} n_{\text{branch}} \} \uplus \{ n_{\text{branch}} \xrightarrow{\text{CF}} n_t^{\text{entry}} \mid n_t^{\text{entry}} \in \text{entries}(G_t) \} \\
\quad \uplus \{ n_{\text{branch}} \xrightarrow{\text{CF}} n_f^{\text{entry}} \mid n_f^{\text{entry}} \in \text{entries}(G_f) \} \\
\quad \uplus \{ o_t(k) \xrightarrow{\text{DF}(k)} n_{\text{merge}} \mid k \in \text{dom}(o_t) \} \\
\quad \uplus \{ o_f(k) \xrightarrow{\text{DF}(k)} n_{\text{merge}} \mid k \in \text{dom}(o_f) \}
$$

The output BD of the merge node is $bd_{\text{merge}} = bd_t \sqcup_{\text{BD}} bd_f$, where $\sqcup_{\text{BD}}$ is the BD join (least upper bound): the RepD is the union of valid interpretations, the CapD is the intersection of capabilities (both branches must agree on what is permitted), and the RelD is the conjunction of relational constraints from both branches.

**BD Consistency Requirement.** Both branches must produce output BDs that are joinable: $bd_t \sqcup_{\text{BD}} bd_f$ must exist. This requires that the two branches have compatible RepDs (same size and alignment) and that their CapDs are not contradictory (if one branch produces a read-only value and the other produces a read-write value, the join is read-only — the intersection).

Conditional composition is the SCG analog of an if-then-else expression. The Branch node creates two disjoint control-flow paths, and the Merge node joins them. The IVE must verify that both paths are well-formed and that the join BD is consistent.

### 3.5 Recursive Composition ($\mu X . G(X)$)

**Definition.** Recursive composition expresses fixed-point iteration in the SCG. Given an SCG template $G(X)$ where $X$ is a "hole" (a designated subgraph port that can be replaced by a recursive reference), the recursive composition $\mu X . G(X)$ constructs:

$$
N' = N_G \uplus \{n_{\text{loop}}, n_{\text{guard}}\}
$$

where $n_{\text{loop}} = \text{ControlNode}(\text{Loop}(\text{body} = G, \text{guard} = n_{\text{guard}}), bd_{\text{loop}})$ and $n_{\text{guard}}$ is a ComputationNode that evaluates the loop continuation condition.

The output BD is computed as the least fixed point of the BD transformer:

$$
bd_{\text{loop}} = \mu \, bd . \; bd_G[bd_{\text{loop}} / bd_X]
$$

where $bd_G[bd_{\text{loop}} / bd_X]$ denotes the BD of $G$ with the hole's BD replaced by $bd_{\text{loop}}$. The fixed point exists because the BD lattice is finite (RepDs have bounded size, CapDs are subsets of a finite capability set, and RelDs are conjunctions of a finite constraint set) and the transformer is monotone (substituting a larger BD for the hole cannot produce a smaller output BD).

**Termination.** The IVE must verify that the guard $n_{\text{guard}}$ is not a tautology (the loop can exit). For total correctness, the IVE must also construct a variant — a natural number that strictly decreases on each iteration — or accept the loop as potentially non-terminating and flag it accordingly.

**Well-formedness of Recursive Composition.** The recursive reference from $G$'s output back to $G$'s input introduces a cycle in the ControlFlow edges (which is permitted, per Section 2.2), but must not introduce a cycle in the DataFlow edges. The data produced by one iteration must flow into the next iteration via the loop's carry variables, which are routed through the Merge node at the loop entry.

---

## 4. Equivalence Relation — Bisimulation

### 4.1 Definition: SCG Bisimulation

Two SCGs $G_1 = (N_1, E_1, \iota_1, o_1)$ and $G_2 = (N_2, E_2, \iota_2, o_2)$ are **bisimilar** (written $G_1 \sim G_2$) if there exists a relation $R \subseteq N_1 \times N_2$ satisfying the following conditions:

**Input correspondence.** For each input port $k \in \text{dom}(\iota_1) = \text{dom}(\iota_2)$:

$$
(\iota_1(k), \iota_2(k)) \in R
$$

**Output correspondence.** For each output port $k \in \text{dom}(o_1) = \text{dom}(o_2)$:

$$
(o_1(k), o_2(k)) \in R
$$

**Node kind preservation.** For all $(n_1, n_2) \in R$:

$$
\text{kind}(n_1) = \text{kind}(n_2)
$$

**BD compatibility.** For all $(n_1, n_2) \in R$:

$$
\text{outBD}(n_1) \equiv_{\text{BD}} \text{outBD}(n_2)
$$

where $\equiv_{\text{BD}}$ denotes BD equivalence: two BDs are equivalent if they describe the same behavioral properties (same RepD interpretations, same CapD, logically equivalent RelD). This is weaker than syntactic equality of BD representations — two structurally different RelD conjunctions may be logically equivalent.

**DataFlow zig-zag.** For all $(n_1, n_2) \in R$, if $n_1 \xrightarrow{\text{DF}(\pi)} m_1$ in $G_1$, then there exists $m_2 \in N_2$ such that $n_2 \xrightarrow{\text{DF}(\pi)} m_2$ in $G_2$ and $(m_1, m_2) \in R$. Symmetrically, if $n_2 \xrightarrow{\text{DF}(\pi)} m_2$ in $G_2$, then there exists $m_1 \in N_1$ such that $n_1 \xrightarrow{\text{DF}(\pi)} m_1$ in $G_1$ and $(m_1, m_2) \in R$.

**ControlFlow zig-zag.** For all $(n_1, n_2) \in R$, if $n_1 \xrightarrow{\text{CF}} m_1$ in $G_1$, then there exists $m_2 \in N_2$ such that $n_2 \xrightarrow{\text{CF}} m_2$ in $G_2$ and $(m_1, m_2) \in R$ (and symmetrically). This ensures that the control-flow structure is preserved.

**Derivation zig-zag.** For all $(n_1, n_2) \in R$, if $n_1 \xrightarrow{\text{Deriv}} m_1$ in $G_1$, then there exists $m_2 \in N_2$ such that $n_2 \xrightarrow{\text{Deriv}} m_2$ in $G_2$ and $(m_1, m_2) \in R$ (and symmetrically). This ensures that the pointer derivation structure is preserved — critical for VUMA memory safety reasoning.

**Annotation zig-zag.** For all $(n_1, n_2) \in R$, if $n_1 \xrightarrow{\text{Ann}} m_1$ in $G_1$, then there exists $m_2 \in N_2$ such that $n_2 \xrightarrow{\text{Ann}} m_2$ in $G_2$ and $(m_1, m_2) \in R$ (and symmetrically). This ensures that behavioral annotations are preserved.

### 4.2 Bisimulation is an Equivalence Relation

**Theorem.** The bisimulation relation $\sim$ is an equivalence relation on SCGs.

**Proof.** We verify the three properties:

*Reflexivity.* For any SCG $G = (N, E, \iota, o)$, the identity relation $R = \{(n, n) \mid n \in N\}$ is a bisimulation. All zig-zag conditions are trivially satisfied because each node maps to itself. Input and output correspondence hold by definition. BD compatibility holds because $\text{outBD}(n) \equiv_{\text{BD}} \text{outBD}(n)$ is trivially true. Hence $G \sim G$.

*Symmetry.* If $G_1 \sim G_2$ via relation $R$, then $G_2 \sim G_1$ via relation $R^{-1} = \{(n_2, n_1) \mid (n_1, n_2) \in R\}$. The zig-zag conditions are symmetric by construction: if every forward step in $G_1$ has a matching step in $G_2$ via $R$, then every forward step in $G_2$ has a matching step in $G_1$ via $R^{-1}$. BD equivalence is symmetric because $\equiv_{\text{BD}}$ is symmetric. Hence $G_1 \sim G_2 \implies G_2 \sim G_1$.

*Transitivity.* If $G_1 \sim G_2$ via $R_{12}$ and $G_2 \sim G_3$ via $R_{23}$, then $G_1 \sim G_3$ via $R_{13} = R_{12} \circ R_{23} = \{(n_1, n_3) \mid \exists n_2. (n_1, n_2) \in R_{12} \wedge (n_2, n_3) \in R_{23}\}$. To verify the zig-zag condition: if $n_1 \xrightarrow{\text{DF}(\pi)} m_1$ and $(n_1, n_3) \in R_{13}$, then there exists $n_2$ with $(n_1, n_2) \in R_{12}$ and $(n_2, n_3) \in R_{23}$. By the zig-zag property of $R_{12}$, there exists $m_2$ with $n_2 \xrightarrow{\text{DF}(\pi)} m_2$ and $(m_1, m_2) \in R_{12}$. By the zig-zag property of $R_{23}$, there exists $m_3$ with $n_3 \xrightarrow{\text{DF}(\pi)} m_3$ and $(m_2, m_3) \in R_{23}$. Then $(m_1, m_3) \in R_{13}$. The same argument applies to ControlFlow, Derivation, and Annotation edges. BD compatibility is transitive because $\equiv_{\text{BD}}$ is an equivalence relation on behavioral descriptors (which follows from the symmetry and transitivity of logical equivalence on RelD constraints). Hence $G_1 \sim G_2 \wedge G_2 \sim G_3 \implies G_1 \sim G_3$. $\square$

### 4.3 Canonical Form

**Definition.** A **canonical form** of an SCG $G$ is a representative $\text{canon}(G)$ of its bisimulation equivalence class $[G]_{\sim}$ such that for any $G_1, G_2$:

$$
G_1 \sim G_2 \iff \text{canon}(G_1) = \text{canon}(G_2)
$$

where equality denotes syntactic (structural) identity of the graph representation.

**Construction sketch.** The canonical form is obtained by:

1. **Minimization.** Compute the maximal bisimulation on $G$ (the union of all bisimulation relations from $G$ to itself) using a partition-refinement algorithm analogous to Paige-Tarjan minimization for labeled transition systems. This identifies all bisimulation-equivalent node pairs within $G$.

2. **Quotient.** Form the quotient graph $G / {\sim}$ by collapsing each bisimulation-equivalence class of nodes into a single representative node. Edges are rewritten to connect the representative nodes. The result is the bisimulation-minimal graph that is behaviorally equivalent to $G$.

3. **Normalization.** Apply a deterministic ordering to: (a) node identifiers (assigned in a canonical order based on the topological sort of the DataFlow DAG, breaking ties by node kind, then by BD hash), (b) edge ordering (sorted by source node, then label, then target node), and (c) BD representation (choose a canonical form for RelD constraints, e.g., by reducing to conjunctive normal form and sorting clauses lexicographically).

4. **Hash.** Compute a structural hash of the normalized quotient graph. Two canonical forms are identical if and only if their structural hashes match and their graph structures are isomorphic under the canonical node ordering.

**Why it exists.** The canonical form exists because: (a) the bisimulation equivalence classes partition the space of SCGs into finite sets (for any finite SCG, the quotient is finite); (b) the normalization procedure is deterministic (given a total order on node kinds, BD hashes, and topological positions, the ordering is uniquely determined); and (c) the resulting canonical representation is unique up to the choice of normalization conventions, which are fixed by this specification. The canonical form is the foundation for the SCG's "unique canonical form" property described in the proposal (Section 3.1): two programs with the same semantics produce the same canonical SCG, regardless of how they were constructed.

---

## 5. Well-Formedness Conditions

### 5.1 Definition: Well-Formed SCG

An SCG $G = (N, E, \iota, o)$ is **well-formed** (written $\mathsf{wellFormed}(G)$) if and only if it satisfies the following four conditions simultaneously. These conditions are the minimal set of structural invariants that an SCG must satisfy for the IVE to reason about it correctly and for the VUMA memory model to guarantee safety.

### 5.2 Condition 1: Derivation Completeness (Every Access Has an Origin)

**Statement.** For every AccessNode $n_a \in N$, there exists an AllocationNode $n_{\text{alloc}} \in N$ and a Derivation path from $n_{\text{alloc}}$ to $n_a$:

$$
\forall n_a \in N .\; \text{kind}(n_a) = \text{Access} \implies \exists n_{\text{alloc}} \in N .\; \text{kind}(n_{\text{alloc}}) = \text{Alloc} \wedge n_{\text{alloc}} \xrightarrow{\text{Deriv}^*} n_a
$$

where $\xrightarrow{\text{Deriv}^*}$ denotes the reflexive-transitive closure of the Derivation edge relation.

**Rationale.** This condition is the SCG structural encoding of VUMA's origin invariant (Proposal Section 3.6.2): every memory access must be traceable to a valid allocation. If an AccessNode has no Derivation path to any AllocationNode, then the address being accessed was either computed from a literal constant (a potential invalid-memory access) or derived from an untracked source (a verification gap). The IVE flags any violation of this condition as a critical safety error. Note that the Derivation path may pass through intermediate ComputationNodes (pointer arithmetic, offset computation), each of which propagates the Derivation chain. The Derivation subgraph must be a forest rooted at AllocationNodes — every node with an incoming Derivation edge has exactly one predecessor in the Derivation subgraph, ensuring that the origin is unambiguous.

**Verification complexity.** Checking Derivation completeness requires a reachability query on the Derivation subgraph for each AccessNode. This can be performed in $O(|N| + |E_{\text{Deriv}}|)$ time using a union-find data structure where each AllocationNode initializes a set and Derivation edges merge sets. If an AccessNode's address input is not in any allocation's set, the condition is violated.

### 5.3 Condition 2: DataFlow Acyclicity (DAG Property)

**Statement.** The DataFlow subgraph of $G$ is a directed acyclic graph:

$$
\nexists \; n_0 \xrightarrow{\text{DF}} n_1 \xrightarrow{\text{DF}} \cdots \xrightarrow{\text{DF}} n_0 \quad \text{in } E_{\text{DF}}
$$

Formally, the relation $\xrightarrow{\text{DF}^+}$ (the transitive closure of DataFlow edges) is irreflexive: $n \not\xrightarrow{\text{DF}^+} n$ for all $n \in N$.

**Rationale.** The DataFlow DAG property ensures that the SCG has a well-defined evaluation order: values flow from producers to consumers without circular dependencies. This is essential for the IVE to perform type inference (which proceeds in topological order), for the COR to schedule computation (which requires a dependency-aware execution plan), and for the VUMA memory model to determine liveness at each program point (which requires a partial order on node execution). Cycles in DataFlow would represent ill-defined computations — a value that depends on itself without a fixed-point operator. Note that cycles are permitted in the ControlFlow subgraph (loops), and the recursive composition operator ($\mu X . G(X)$) introduces such cycles. However, the data flowing *through* a loop iteration is not cyclic: the output of one iteration feeds the input of the *next* iteration via the Merge node at the loop entry, which is a sequential DataFlow path, not a cycle.

**Verification complexity.** Acyclicity can be checked by a topological sort of the DataFlow subgraph in $O(|N| + |E_{\text{DF}}|)$ time. If the sort fails (not all nodes can be placed in a topological order), a DataFlow cycle exists.

### 5.4 Condition 3: Input Satisfaction (Every Node's Inputs Are Provided)

**Statement.** For every node $n \in N$ and every required input port $\pi$ of $n$, there exists exactly one DataFlow edge providing a value at port $\pi$:

$$
\forall n \in N .\; \forall \pi \in \text{inputPorts}(n) .\; |\{ e \in \text{in}_{\text{DF}}(n) \mid \text{port}(e) = \pi \}| = 1
$$

where $\text{inputPorts} : \mathcal{N} \to \mathcal{P}(\mathbb{N})$ maps each node to its set of required input port indices, determined by its kind and parameters:
- $\text{inputPorts}(\text{ComputationNode}(f, [v_1, \ldots, v_k], bd)) = \{1, \ldots, k\}$  
- $\text{inputPorts}(\text{AccessNode}(a, m, bd)) = \{1\}$ (the address input) plus $\{2\}$ if $m = \text{Write}$ (the value to write)  
- $\text{inputPorts}(\text{CastNode}(bd_s, bd_t, bd)) = \{1\}$ (the value to cast)  
- $\text{inputPorts}(\text{DeallocationNode}(r^*, bd)) = \{1\}$ (the region reference)  
- $\text{inputPorts}(\text{ControlNode}(\text{Branch}(v), bd)) = \{1\}$ (the condition value)  
- $\text{inputPorts}(\text{ControlNode}(\text{Merge}(k), bd)) = \{1, \ldots, k\}$  
- $\text{inputPorts}(\text{ControlNode}(\text{Loop}(b, g), bd)) = \{1\}$ (the initial loop carry value)  
- $\text{inputPorts}(\text{AllocationNode}(r, bd)) = \emptyset$  
- $\text{inputPorts}(\text{EffectNode}(e, bd)) = \{1\}$ (the effect's data input)  
- $\text{inputPorts}(\text{PhantomNode}(bd)) = \emptyset$  

**Rationale.** This condition ensures that no node is "dangling" — every computation has its arguments, every access has its address, every cast has its source value. A node with unsatisfied inputs would be semantically undefined: the IVE could not infer its output BD, and the COR could not execute it. The "exactly one" constraint (rather than "at least one") prevents ambiguous inputs where multiple producers feed the same port, which would create a data race unless the multiplicity is resolved by a Merge node.

**Verification complexity.** Input satisfaction can be checked in $O(|E_{\text{DF}}| + |N|)$ time by iterating over all DataFlow edges, grouping by target node and port, and verifying that each required port has exactly one incoming edge.

### 5.5 Condition 4: Type Consistency (All Edges Carry BD-Annotated Values)

**Statement.** For every DataFlow edge $e = n_s \xrightarrow{\text{DF}(\pi)} n_t$, the BD of the value produced by $n_s$ is compatible with the BD expected by $n_t$ at input port $\pi$:

$$
\forall e = n_s \xrightarrow{\text{DF}(\pi)} n_t \in E_{\text{DF}} .\; \text{outBD}(n_s) \sqsubseteq_{\text{BD}} \text{expectedBD}(n_t, \pi)
$$

where $\text{expectedBD}(n_t, \pi)$ is the BD that $n_t$ requires at its $\pi$-th input port (determined by the function signature for ComputationNodes, the access mode for AccessNodes, etc.), and $\sqsubseteq_{\text{BD}}$ is the BD compatibility relation defined in Section 3.2.

Additionally, for every Annotation edge $n_s \xrightarrow{\text{Ann}} n_t$:

$$
\text{outBD}(n_s).\text{CapD} \subseteq \text{outBD}(n_t).\text{CapD}
$$

(annotations can only restrict capabilities, not expand them), and:

$$
\text{outBD}(n_s).\text{RelD} \implies \text{outBD}(n_t).\text{RelD}
$$

(the annotation's relational constraints must be consistent with the target's — the annotation can add constraints but not contradict existing ones).

**Rationale.** Type consistency is the SCG analog of a traditional type checker, but operating on BDs rather than nominal types. This condition ensures that every value flowing through the graph carries sufficient behavioral information for the consumer to operate correctly. If a ComputationNode expects a value with CapD containing $\{\text{read}\}$, and the producer's BD only includes $\{\text{write}\}$, then the DataFlow edge is ill-typed — the consumer would attempt to read a value that does not permit reading. The IVE uses this condition to verify the entire program's behavioral consistency, which subsumes traditional type checking, borrow checking, and security-level verification. The BD compatibility relation $\sqsubseteq_{\text{BD}}$ is the generalization of subtyping: it permits capability narrowing (a read-write value can flow into a read-only context) and representation refinement (a value with a more specific RepD can flow into a context expecting a more general one), but not the reverse.

**Verification complexity.** Type consistency requires checking one BD compatibility relation per DataFlow edge. Each check involves: (a) RepD layout compatibility (size and alignment comparison), (b) CapD subset check, and (c) RelD logical consistency check (satisfiability of the conjunction of annotation and target constraints). Steps (a) and (b) are $O(1)$; step (c) is NP-hard in general (propositional satisfiability), but in practice, RelD constraints are maintained in a restricted form (conjunctions of atomic predicates) that admits polynomial-time consistency checking. The total verification cost is $O(|E_{\text{DF}}| \cdot c_{\text{BD}})$ where $c_{\text{BD}}$ is the cost of a single BD compatibility check.

### 5.6 Composite Well-Formedness Theorem

**Theorem.** If $G$ is well-formed ($\mathsf{wellFormed}(G)$) and $G'$ is derived from $G$ by any semantics-preserving graph transformation $T$ (i.e., $G \sim T(G)$), then $\mathsf{wellFormed}(T(G))$.

**Proof sketch.** Every semantics-preserving transformation preserves the four well-formedness conditions by construction:
- Derivation completeness is preserved because bisimulation requires the Derivation zig-zag property (any AccessNode in $T(G)$ has a corresponding AccessNode in $G$ with a Derivation path, and the path is preserved by the transformation).
- DataFlow acyclicity is preserved because bisimulation requires the DataFlow zig-zag property (a cycle in $T(G)$'s DataFlow would imply a cycle in $G$'s DataFlow, since the zig-zag maps edges in both directions).
- Input satisfaction is preserved because bisimulation requires that every DataFlow edge in $G$ has a corresponding edge in $T(G)$ with the same port assignment.
- Type consistency is preserved because bisimulation requires BD compatibility at every corresponding edge.

This theorem is the formal guarantee that the IVE's optimization and refactoring transformations (which are all semantics-preserving) never introduce well-formedness violations. It is the SCG's replacement for the traditional compiler's invariant that "optimizations never introduce type errors." $\square$

---

## Appendix A: Summary of Formal Objects

| Object | Domain | Description |
|--------|--------|-------------|
| $\mathcal{N}$ | ADT (8 constructors) | Universe of SCG nodes |
| $\mathcal{E}$ | $\mathcal{N} \times \mathcal{L} \times \mathcal{N}$ | Universe of SCG edges |
| $\mathcal{L}$ | $\{\text{DF}, \text{CF}, \text{Deriv}, \text{Ann}\}$ | Edge labels |
| $\mathcal{BD}$ | $\text{RepD} \times \text{CapD} \times \text{RelD}$ | Behavioral Descriptor |
| $G$ | $(N, E, \iota, o)$ | SCG tuple |
| $\mathbin{;}$ | $\text{SCG} \times \text{SCG} \to \text{SCG}$ | Sequential composition |
| $\parallel$ | $\text{SCG} \times \text{SCG} \to \text{SCG}$ | Parallel composition |
| $\text{cond}$ | $\text{SCG}^3 \to \text{SCG}$ | Conditional composition |
| $\mu$ | $(\text{SCG} \to \text{SCG}) \to \text{SCG}$ | Recursive composition (fixed-point) |
| $\sim$ | $\mathcal{P}(\text{SCG} \times \text{SCG})$ | Bisimulation equivalence |
| $\text{canon}$ | $\text{SCG} \to \text{SCG}$ | Canonical form function |
| $\mathsf{wellFormed}$ | $\text{SCG} \to \mathbb{B}$ | Well-formedness predicate |

## Appendix B: Open Questions

1. **BD equivalence decidability.** The $\equiv_{\text{BD}}$ relation on RelDs requires logical equivalence of constraint conjunctions, which is undecidable in general (it subsumes first-order satisfiability). A restricted RelD language (e.g., quantifier-free, finite-domain) would restore decidability. The exact restriction is an open design decision.

2. **Canonical form computation complexity.** The partition-refinement step of canonicalization is $O(|E| \log |N|)$ for labeled transition systems, but the BD comparison and RelD normalization steps may increase this. Bounding the complexity of canonical form computation is an open problem.

3. **Recursive composition fixed-point convergence.** The fixed-point $\mu \, bd . \, bd_G[bd_{\text{loop}} / bd_X]$ is guaranteed to converge on the finite BD lattice, but the rate of convergence may be exponential in the depth of the RelD constraint system. Practical convergence guarantees for the IVE remain to be established.

4. **Well-formedness under incremental editing.** When the COR incrementally recompiles a subgraph after an edit, it must verify that the edit does not violate well-formedness. Incremental well-formedness checking algorithms (analogous to incremental type checking) are a topic for future work.

---

*End of specification.*
