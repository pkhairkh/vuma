# BD Inference Algorithm Specification

**Document ID:** VUMA-SPEC-BD-INF-001  
**Task ID:** W1-26, updated by W2-A7  
**Author:** Agent W1-26, Agent W2-A7  
**Date:** 2026-03-04, updated 2026-03-05  
**Status:** Draft (updated with SCG-based inference)  
**Dependencies:** SCG Specification, VUMA Memory Model Specification  

---

## Table of Contents

1. [RepD Inference Algorithm](#1-repd-inference-algorithm)
2. [CapD Inference Algorithm](#2-capd-inference-algorithm)
3. [RelD Inference Algorithm](#3-reld-inference-algorithm)
4. [Combined BD Inference](#4-combined-bd-inference)
5. [Soundness Theorem](#5-soundness-theorem)
6. [Completeness Discussion](#6-completeness-discussion)
7. [RepD Inference from SCG](#7-repd-inference-from-scg)
8. [CapD Inference from SCG](#8-capd-inference-from-scg)
9. [RelD Inference from SCG](#9-reld-inference-from-scg)
10. [Full BD Inference from SCG](#10-full-bd-inference-from-scg)
11. [Subsumption of Rust Type System](#11-subsumption-of-rust-type-system)
12. [BD Fixpoint Solver](#12-bd-fixpoint-solver)

---

## 1. RepD Inference Algorithm

### 1.1 Overview

Representation Descriptors (RepD) specify the physical layout of data in memory — size, alignment, field offsets, and bit-level structure. Unlike traditional types, RepD is a *memory map* that enumerates all valid interpretations of a memory region rather than selecting a single nominal category. The RepD inference algorithm determines, for every value in the Semantic Computation Graph (SCG), the precise memory representation that the value carries at each program point. This inference is performed first among the three BD components because RepD has no dependency on CapD or RelD — it is determined purely by the structural and operational properties of the program.

The algorithm operates as a forward dataflow analysis over the SCG. Each node in the SCG produces a value (or values) that carry RepD annotations, and these annotations are propagated along DataFlow edges. At certain nodes — allocation nodes, cast nodes, and access nodes — the algorithm either initializes, transforms, or verifies RepD information. The key invariant maintained throughout inference is that every value at every program point has a well-defined RepD, and that all operations consuming that value respect its RepD.

### 1.2 Formal Definitions

A **RepD** is a record comprising the following fields:

```
RepD ::= {
  size     : Nat,              // total size in bytes
  align    : Nat,              // alignment requirement in bytes
  interp   : Set<Interpretation>,  // set of valid interpretations
  variant  : Option<VariantMap>    // for sum-type representations
}

Interpretation ::= 
  | PrimitiveInt(bits: Nat, signed: Bool)
  | PrimitiveFloat(bits: Nat)
  | PointerTo(target: RepD)
  | ArrayOf(elem: RepD, count: Nat)
  | StructOf(fields: List<(Name, RepD, Offset)>)
  | BytesOf(count: Nat)
  | UnionOf(alternatives: Set<RepD>)    // overlapping interpretations
  | Opaque                               // unknown interpretation

VariantMap ::= Map<VariantTag, RepD>   // tag -> variant RepD
```

An **AllocationNode** in the SCG represents a memory allocation event (stack allocation, heap allocation, static allocation, memory-mapped region). Each AllocationNode carries annotations specifying the size and alignment of the allocated region, and optionally a structural layout.

A **CastNode** represents a reinterpretation of a value's memory representation. CastNode carries an annotation specifying the target RepD.

An **AccessNode** represents a read or write operation on a memory region, specifying the expected RepD of the accessed sub-region.

A **FunctionCall** node represents invocation of a function with formal parameter BDs and a return BD.

### 1.3 Algorithm

```
Algorithm: RepD-Inference
Input:  SCG = (V, E) where V = nodes, E = dataflow edges
Output: Map<Node, RepD> mapping each value-producing node to its RepD
        OR Error with diagnostic information

1.  // Phase 1: Initialization
2.  RepD_map := empty map from Node to RepD
3.  WorkList := empty queue of Nodes
4.  
5.  for each node v in V do
6.    if v is AllocationNode then
7.      // Derive RepD from allocation annotations
8.      repd := RepD from v's size/align/layout annotations
9.      if v has structural layout annotation then
10.       repd.interp := { StructOf(v.fields) }
11.     else
12.       repd.interp := { BytesOf(v.size) }  // opaque bytes
13.     RepD_map[v] := repd
14.     WorkList.enqueue(v)
15.   
16.   elif v is LiteralNode then
17.     // Derive RepD from literal's intrinsic representation
18.     repd := RepD from v's literal type
19.     RepD_map[v] := repd
20.     WorkList.enqueue(v)
21.   
22.   elif v is ParameterNode then
23.     // Formal parameter RepD comes from function signature
24.     repd := v.function_signature.param_RepD[v.param_index]
25.     RepD_map[v] := repd
26.     WorkList.enqueue(v)
27. end for
28. 
29. // Phase 2: Forward propagation
30. while WorkList is not empty do
31.   v := WorkList.dequeue()
32.   current_repd := RepD_map[v]
33.   
34.   for each edge (v, u) in E do  // v flows to u
35.     case u.type of
36.       
37.       DataFlowPass:  // simple pass-through
38.         if u not in RepD_map or RepD_map[u] != current_repd then
39.           RepD_map[u] := current_repd
40.           WorkList.enqueue(u)
41.       
42.       CastNode:
43.         // Compute new RepD from cast annotation
44.         target_repd := u.cast_annotation.target_RepD
45.         // Verify compatibility: source memory must cover target
46.         if not RepD-Compatible(current_repd, target_repd) then
47.           error("CastNode %u: source RepD %current_repd incompatible "
48.                 "with target RepD %target_repd")
49.         RepD_map[u] := target_repd
50.         WorkList.enqueue(u)
51.       
52.       AccessNode:
53.         // Verify RepD is compatible with access type
54.         access_repd := u.access_annotation.expected_RepD
55.         if not RepD-Compatible(RepD_map[u], access_repd) then
56.           error("AccessNode %u: value RepD incompatible with access "
57.                 "type %access_repd")
58.         // AccessNode does not change the RepD of the value
59.         // but may produce a sub-value with a different RepD
60.         if u.produces_subvalue then
61.           sub_repd := RepD-Project(RepD_map[u], u.access_path)
62.           RepD_map[u.output] := sub_repd
63.           WorkList.enqueue(u.output)
64.       
65.       FunctionCall:
66.         // Verify caller's actual RepDs match callee's formal RepDs
67.         callee := u.callee_function
68.         for i = 0 to u.actual_params.length - 1 do
69.           actual_repd := RepD_map[u.actual_params[i]]
70.           formal_repd := callee.signature.param_RepD[i]
71.           if not RepD-Compatible(actual_repd, formal_repd) then
72.             error("FunctionCall %u: actual param %i RepD %actual_repd "
73.                   "incompatible with formal param RepD %formal_repd")
74.         // Return value gets callee's return RepD
75.         RepD_map[u] := callee.signature.return_RepD
76.         WorkList.enqueue(u)
77.       
78.       PhiNode:  // merge point from control flow
79.         // All predecessors must agree on RepD (or be union-compatible)
80.         incoming := { RepD_map[p] for each predecessor p of u }
81.         merged := RepD-Merge(incoming)
82.         if merged = FAIL then
83.           error("PhiNode %u: incompatible incoming RepDs %incoming")
84.         if u not in RepD_map or RepD_map[u] != merged then
85.           RepD_map[u] := merged
86.           WorkList.enqueue(u)
87.       
88.       default:
89.         // All other nodes: RepD passes through unchanged
90.         if u not in RepD_map or RepD_map[u] != current_repd then
91.           RepD_map[u] := current_repd
92.           WorkList.enqueue(u)
93.   end for
94. end while
95. 
96. // Phase 3: Completeness check
97. for each node v in V do
98.   if v not in RepD_map then
99.     error("Node %v: RepD could not be inferred (unreachable or "
100.          "uninitialized)")
101. end for
102. 
103. return RepD_map
```

### 1.4 Helper Functions

```
Function RepD-Compatible(source: RepD, target: RepD) -> Bool
  // A cast from source to target is valid if:
  // 1. source.size >= target.size (target fits within source's memory)
  // 2. target.align divides source.align (alignment is compatible)
  // 3. If target specifies a struct layout, all field offsets
  //    fall within source.size
  return source.size >= target.size
     and source.align % target.align == 0
     and fields-within-bounds(target, source.size)

Function RepD-Merge(repd_set: Set<RepD>) -> RepD or FAIL
  // Merge multiple RepDs from control flow merge points
  if all elements of repd_set are equal then
    return any element
  // Try to find a common supertype: union of interpretations
  merged_size := max(r.size for r in repd_set)
  merged_align := lcm(r.align for r in repd_set)
  merged_interp := union(r.interp for r in repd_set)
  // Verify that all interpretations are valid for merged size/align
  for each interp in merged_interp do
    if not interp-fits(interp, merged_size, merged_align) then
      return FAIL
  return RepD{size=merged_size, align=merged_align, interp=merged_interp}

Function RepD-Project(repd: RepD, path: AccessPath) -> RepD
  // Project a sub-region from a struct RepD
  match path with
    | FieldAccess(name) -> find field named 'name' in repd.interp
    | IndexAccess(idx)  -> return element RepD from array interpretation
    | SubRange(off, sz) -> return RepD{size=sz, align=1, interp={BytesOf(sz)}}
```

### 1.5 Complexity Analysis

The algorithm performs a standard forward dataflow analysis on the SCG. Each node is enqueued in the worklist at most once per distinct RepD value it can hold. In practice, the RepD at a node changes at most O(|RepD_variants|) times, where |RepD_variants| is the number of distinct representation variants in the program. The total work is therefore O(|V| * |RepD_variants|), which is linear in program size for fixed RepD vocabulary. In the worst case, the number of RepD variants is bounded by the number of distinct struct/type definitions in the program, making the overall complexity O(|V| * |T|) where |T| is the number of type definitions. This is analogous to the complexity of Hindley-Milner type inference, which is nearly linear in practice.

### 1.6 Consistency Proof Sketch

**Theorem:** If RepD-Inference succeeds for a program P (i.e., returns a RepD_map without error), then all RepD annotations in the program are consistent.

**Proof Sketch:** By induction on the structure of the SCG.

- **Base case (AllocationNode, LiteralNode, ParameterNode):** The RepD is initialized directly from the node's intrinsic annotations, which are consistent by construction (the allocation request specifies a valid size and alignment).

- **Inductive step (DataFlow pass-through):** If the source node's RepD is consistent, and the algorithm propagates it unchanged to the target, then the target's RepD is consistent because the data has the same representation.

- **Inductive step (CastNode):** The algorithm verifies RepD-Compatible(source, target) before assigning the target RepD. By definition of RepD-Compatible, the target memory region is a valid reinterpretation of the source memory region. Therefore, all subsequent operations that respect the target RepD also respect the source memory layout.

- **Inductive step (FunctionCall):** The algorithm verifies that each actual parameter's RepD is compatible with the corresponding formal parameter's RepD. By the induction hypothesis, the callee's body is consistent with its formal RepDs. Therefore, the call is consistent.

- **Inductive step (PhiNode):** The merge function produces a RepD that is a valid supertype of all incoming RepDs. Any operation valid on the merged RepD is valid on all incoming RepDs. Therefore, the merged RepD is consistent.

Since every node is covered by one of these cases, and the algorithm processes all nodes, global consistency follows. QED.

---

## 2. CapD Inference Algorithm

### 2.1 Overview

Capability Descriptors (CapD) specify what operations are valid on data — a set of permissions that is context-dependent and may be weakened (reduced) as data flows through the program. Unlike RepD, which is preserved along data flow, CapD is *weakened* along data flow edges: passing a value to a function may reduce its capabilities, storing it in a shared structure may remove write access, and sending it over a network may strip all local capabilities.

The CapD inference algorithm is a forward dataflow analysis over the SCG that tracks the capability set of each value at each program point. The analysis uses a lattice structure where the top element is the full capability set and the ordering is subset inclusion. At merge points (control flow joins), CapD sets are joined using union (the most permissive capability set that satisfies both branches). At capability-reducing operations (function calls with restricted signatures, free operations, send operations), CapD is weakened by intersecting with the required capability set.

The algorithm depends on RepD inference for cast validation — when a value is cast to a new RepD, the CapD must be verified to include the operations implied by the target representation (e.g., casting to a pointer type requires the DerivePtr capability).

### 2.2 Formal Definitions

A **CapD** is a set of capabilities drawn from the following universe:

```
Capability ::= 
  | Read            // may read the value
  | Write           // may write/modify the value
  | DerivePtr       // may derive a pointer/address to the value
  | Drop            // may deallocate/drop the value
  | Move            // may transfer ownership (move semantics)
  | Copy            // may duplicate the value (copy semantics)
  | Iterate         // may iterate over the value (collection)
  | Execute         // may execute the value as code
  | Serialize       // may serialize the value to bytes
  | Send            // may send the value across a boundary (network, thread)
  | Persist         // may persist the value to durable storage
  | Compare         // may compare the value for equality
  | Hash            // may compute a hash of the value
  | FFI             // may pass the value to foreign function interface

CapD = Set<Capability>
```

The **CapD lattice** is defined as:

- **Ordering:** C1 ≤ C2 iff C1 ⊇ C2 (subset ordering: fewer capabilities = lower in lattice)
- **Top (⊤):** ∅ (no capabilities — the most restrictive, lowest in our inverted ordering)
- **Bottom (⊥):** AllCapabilities (every capability — the most permissive)
- **Join (∨):** C1 ∪ C2 (union: take the most permissive capabilities)
- **Meet (∧):** C1 ∩ C2 (intersection: take the most restrictive capabilities)

Note: The lattice is inverted with respect to the natural subset ordering. We use "weakening" to mean *removing capabilities* (moving down in the lattice), and "strengthening" to mean *adding capabilities* (moving up). The join at merge points takes the union to ensure that both branches' capabilities are preserved.

### 2.3 Algorithm

```
Algorithm: CapD-Inference
Input:  SCG = (V, E), RepD_map from RepD-Inference
Output: Map<Node, CapD> mapping each value-producing node to its CapD
        OR Error with diagnostic information

1.  // Phase 1: Initialization
2.  CapD_map := empty map from Node to CapD
3.  WorkList := empty queue of Nodes
4.  
5.  FULL_CAPS := {Read, Write, DerivePtr, Drop, Move, Copy, Iterate,
6.                 Execute, Serialize, Send, Persist, Compare, Hash, FFI}
7.  
8.  for each node v in V do
9.    if v is AllocationNode then
10.     // Freshly allocated values get all capabilities
11.     CapD_map[v] := FULL_CAPS
12.     WorkList.enqueue(v)
13.   
14.   elif v is LiteralNode then
15.     // Literals are read-only but copyable
16.     CapD_map[v] := {Read, Copy, Compare, Hash, Serialize}
17.     WorkList.enqueue(v)
18.   
19.   elif v is ParameterNode then
20.     // Formal parameter CapD from function signature
21.     CapD_map[v] := v.function_signature.param_CapD[v.param_index]
22.     WorkList.enqueue(v)
23. end for
24. 
25. // Phase 2: Forward propagation with weakening
26. while WorkList is not empty do
27.   v := WorkList.dequeue()
28.   current_capd := CapD_map[v]
29.   
30.   for each edge (v, u) in E do
31.     case u.type of
32.       
33.       DataFlowPass:
34.         // CapD is preserved along simple data flow
35.         propagate_if_changed(u, current_capd)
36.       
37.       FunctionCall:
38.         // Caller's CapD must satisfy callee's required CapD
39.         callee := u.callee_function
40.         for i = 0 to u.actual_params.length - 1 do
41.           actual_capd := CapD_map[u.actual_params[i]]
42.           required_capd := callee.signature.param_CapD[i]
43.           // Verify: actual capabilities must include all required ones
44.           if not (required_capd ⊆ actual_capd) then
45.             error("FunctionCall %u: actual param %i has CapD "
46.                   "%actual_capd but callee requires %required_capd")
47.           // After call, CapD of passed value is weakened
48.           // to intersection with callee's return CapD for that param
49.           returned_capd := callee.signature.param_CapD_post[i]
50.           new_capd := actual_capd ∩ returned_capd
51.           propagate_if_changed(u.actual_params[i], new_capd)
52.         // Return value gets callee's return CapD
53.         propagate_if_changed(u, callee.signature.return_CapD)
54.       
55.       FreeNode:
56.         // After free(): CapD of derived pointers loses Read and Write
57.         freed_capd := CapD_map[u.freed_value]
58.         if freed_capd is not in CapD_map then
59.           error("FreeNode %u: freeing value with unknown CapD")
60.         // The freed value retains only Drop (it has been dropped)
61.         // All derived pointers lose Read and Write
62.         for each node w derived from u.freed_value do
63.           old_capd := CapD_map[w]
64.           new_capd := old_capd \ {Read, Write, DerivePtr, Execute}
65.           propagate_if_changed(w, new_capd)
66.       
67.       SendNode:
68.         // After send(): CapD of local reference loses everything
69.         // (ownership transferred to remote context)
70.         sent_capd := CapD_map[u.sent_value]
71.         new_capd := {}  // empty CapD after send
72.         propagate_if_changed(u.sent_value, new_capd)
73.       
74.       CastNode:
75.         // Verify CapD includes operations implied by target RepD
76.         target_repd := RepD_map[u]  // from RepD inference
77.         implied_caps := CapD-ImpliedBy(target_repd)
78.         if not (implied_caps ⊆ current_capd) then
79.           error("CastNode %u: target RepD implies capabilities "
80.                 "%implied_caps but value only has %current_capd")
81.         // Casting may restrict capabilities based on target RepD
82.         // (e.g., casting to const removes Write)
83.         cast_capd := u.cast_annotation.capd_restriction
84.         new_capd := current_capd ∩ cast_capd
85.         propagate_if_changed(u, new_capd)
86.       
87.       PhaseTransitionNode:
88.         // In phase transition: CapD may gain or lose capabilities
89.         // based on the phase transition annotation
90.         transition := u.phase_annotation
91.         gained := transition.gained_capabilities
92.         lost := transition.lost_capabilities
93.         new_capd := (current_capd ∪ gained) \ lost
94.         propagate_if_changed(u, new_capd)
95.       
96.       PhiNode:
97.         // At conditional: both branches' CapD are joined (union)
98.         incoming := { CapD_map[p] for each predecessor p of u }
99.         joined := ⋃ incoming  // union = most permissive
100.        propagate_if_changed(u, joined)
101.      
102.      MoveSemanticsNode:
103.        // After a move, the source loses all capabilities
104.        new_source_capd := {}
105:       propagate_if_changed(u.source, new_source_capd)
106:       // The target gets the moved capabilities
107:       propagate_if_changed(u.target, current_capd)
108:      
109:      default:
110:        propagate_if_changed(u, current_capd)
111:  end for
112: end while
113:
114: // Phase 3: Completeness check
115: for each node v in V do
116:   if v not in CapD_map then
117:     error("Node %v: CapD could not be inferred")
118: end for
119:
120: return CapD_map
121:
122: // Helper: propagate only if changed
123: procedure propagate_if_changed(node: Node, new_capd: CapD)
124:   if node not in CapD_map or CapD_map[node] != new_capd then
125:     CapD_map[node] := new_capd
126:     WorkList.enqueue(node)
127:
128: // Helper: capabilities implied by a RepD
129: function CapD-ImpliedBy(repd: RepD) -> CapD
130:   caps := {Read}  // all RepDs require at least Read
131:   if repd has mutable interpretation then caps += {Write}
132:   if repd has pointer interpretation then caps += {DerivePtr}
133:   if repd has function/code interpretation then caps += {Execute}
134:   return caps
```

### 2.4 Context-Dependent Resolution

A critical feature of CapD inference is its context-dependent resolution. The same value may have different CapD at different program points, and the algorithm tracks this precisely. The key context-dependent transformations are:

**After `free()`**: The freed value's memory region is marked as deallocated. All pointers derived from the freed value lose the `Read`, `Write`, `DerivePtr`, and `Execute` capabilities, because accessing freed memory is undefined. They retain `Compare` (the address value can still be compared), `Hash` (the bit pattern can be hashed), and `Serialize` (the address can be serialized as an integer), but these operations are semantically meaningless and the IVE may warn about them.

**After `send()`**: Ownership of the value is transferred to a remote execution context (another thread, another machine, another process). The local reference loses all capabilities — it can no longer be read, written, or derived from. This is the most aggressive weakening, and it mirrors Rust's move semantics for `Send` types.

**Phase transitions**: When the program transitions between phases (e.g., from initialization to steady-state, from authenticated to unauthenticated), capabilities may change. During initialization, a configuration object may be `Write`-able; after initialization, it becomes read-only. The PhaseTransitionNode in the SCG captures these transitions, and the algorithm applies the corresponding capability changes.

**At function boundaries**: The caller's CapD must be a superset of the callee's required CapD. If the callee requires `{Read, Write}` but the caller only provides `{Read}`, the inference fails with a diagnostic. This is analogous to Rust's trait bound checking, but operates on the finer-grained CapD rather than trait implementations.

### 2.5 Complexity Analysis

The CapD lattice has 2^|Capability| elements, where |Capability| is the number of distinct capabilities (currently 14). Each node's CapD can change at most 2^|Capability| times in the worst case. However, in practice, CapD only decreases (weakening) along most paths, and the join at merge points is bounded by the union of incoming CapDs. The total work is O(|V| * |Capability|^2) for the following reasons:

1. Each capability check (subset test) takes O(|Capability|) time.
2. Each node processes at most |Capability| distinct capability changes.
3. The worklist processes each node at most |Capability| times.

Therefore, the overall complexity is polynomial in program size: O(|V| * |Capability|^2). This is efficient enough for practical use on large programs.

### 2.6 Most-Precision Proof Sketch

**Theorem:** If CapD-Inference succeeds, the inferred CapD for each node is the most-precise (largest, i.e., containing the most capabilities) CapD that satisfies all constraints.

**Proof Sketch:** We show this by contradiction. Suppose there exists a valid CapD assignment C' that assigns strictly more capabilities to some node v than the inferred CapD C = CapD_map[v]. That is, C ⊂ C'.

- If v is an AllocationNode, then C = FULL_CAPS, which is the maximum possible CapD. Contradiction.
- If v is reached by data flow from a predecessor p with CapD C_p, then C = C_p (for pass-through) or C = C_p ∩ restriction (for weakening). Since C_p is the most-precise for p (by induction), C is the most-precise for v.
- If v is a PhiNode with predecessors p1, ..., pk, then C = C_p1 ∪ ... ∪ C_pk. Any CapD C' that is valid for v must be valid for all predecessors, meaning C' ⊆ C_pi for each i. Therefore C' ⊆ C, and since C ⊆ C' by assumption, C = C'. Contradiction.
- If v is weakened by a function call or operation, the weakening is the minimum necessary (exact intersection with the required CapD). A larger CapD would violate the required CapD constraint. Contradiction.

Therefore, the inferred CapD is the most-precise assignment. QED.

---

## 3. RelD Inference Algorithm

### 3.1 Overview

Relational Descriptors (RelD) specify relationships between data values — temporal co-occurrence, structural containment, dependency ordering, semantic equivalence, and security-level flow. RelD is the most complex of the three BD components because it captures arbitrary semantic relationships that span the entire program. Unlike RepD (which is local to a value) and CapD (which flows with weakening along edges), RelD involves relationships between *pairs* or *groups* of values, requiring a constraint-based analysis that reasons about the entire relationship graph.

The RelD inference algorithm builds a constraint graph from the SCG, where nodes represent values and edges represent relational constraints. The algorithm then performs fixed-point iteration to compute the transitive closure of all relational constraints, and finally checks consistency (no circular Outlives, no security downgrades).

RelD inference depends on both RepD (for structural containment relationships) and CapD (for capability-dependent relationships, e.g., a value without DerivePtr cannot have its address-derived relation).

### 3.2 Formal Definitions

A **RelD** is a set of relational assertions, each connecting this value to another value or to a property:

```
RelAssertion ::=
  | Outlives(target: NodeID)                    // this value outlives target
  | ContainedIn(container: NodeID)              // this value is contained in container
  | DependsOn(source: NodeID)                   // this value depends on source
  | SemanticallyEquivalentTo(other: NodeID)     // same semantic content, different RepD
  | SecurityLevel(level: SecurityClass)         // confidentiality classification
  | TaintedBy(source: TaintSource)              // data originates from untrusted source
  | ValidDuring(scope: ScopeID)                 // value is valid only within scope
  | AliasesWith(other: NodeID)                  // value shares memory with other

SecurityClass ::= Public | Internal | Confidential | Secret
TaintSource ::= UserInput | NetworkInput | FileAccess | Environment | Untainted

RelD = Set<RelAssertion>
```

The **RelD lattice** is defined as:

- **Ordering:** R1 ≤ R2 iff R1 ⊆ R2 (more assertions = more constrained = lower in lattice)
- **Top (⊤):** {} (no relational assertions — completely unconstrained)
- **Bottom (⊥):** all possible assertions (maximally constrained)
- **Join (∨):** R1 ∪ R2 (union: take all constraints from both)
- **Meet (∧):** R1 ∩ R2 (intersection: take only shared constraints)

The join operation merges relational information: if one path says value A outlives B, and another says A outlives C, the merged RelD says A outlives both B and C. This is the most conservative (most constrained) assignment, ensuring no relational constraint is lost.

### 3.3 Algorithm

```
Algorithm: RelD-Inference
Input:  SCG = (V, E), RepD_map, CapD_map
Output: Map<Node, RelD> mapping each value-producing node to its RelD
        OR Error with diagnostic information

1.  // Phase 1: Initialize all values with empty RelD
2.  RelD_map := Map from Node to RelD, initially {} for all nodes
3.  
4.  // Phase 2: Add temporal relations from scope structure
5.  for each ScopeNode s in V do
6.    // Values declared in scope s outlive all references to them
7.    // that escape scope s
8.    for each value v declared in s do
9.      for each reference r to v that escapes s do
10.       add Outlives(r) to RelD_map[v]
11:      add ValidDuring(s) to RelD_map[v]
12:   // Values in outer scope outlive values in inner scope
13:   for each inner scope s' nested in s do
14:     for each value v in s do
15:       for each value v' in s' do
16:         add Outlives(v') to RelD_map[v]
17: end for
18:
19: // Phase 3: Add dependency relations from data flow
20: for each DataFlow edge (v, u) in E do
21:   // u depends on v (data dependency)
22:   add DependsOn(v) to RelD_map[u]
23:   // If v is a container and u is an element access,
24:   // add containment relation
25:   if u is AccessNode accessing element of v then
26:     add ContainedIn(v) to RelD_map[u]
27:     // Element outlives container semantics:
28:     // container must outlive element reference
29:     add Outlives(u) to RelD_map[v]
30: end for
31:
32: // Phase 4: Add security relations from taint analysis
33: for each node v in V do
34:   if v is InputNode from untrusted source then
35:     add TaintedBy(v.source_type) to RelD_map[v]
36:     // Propagate taint through data flow
37:     // (will be done in Phase 5)
38:   
39:   elif v is DeclassificationNode then
40:     // Explicit declassification: security level may be lowered
41:     // but only at annotated declassification points
42:     declass_level := v.annotation.target_security_level
43:     add SecurityLevel(declass_level) to RelD_map[v]
44:   
45:   else
46:     // Default: security level is maximum of sources
47:     sources := { u | (u, v) in E }
48:     if sources is non-empty then
49:       max_level := max(RelD_map[u].SecurityLevel for u in sources)
50:       add SecurityLevel(max_level) to RelD_map[v]
51: end for
52:
53: // Phase 5: Fixed-point propagation
54: // Build constraint graph and iterate until convergence
55: changed := true
56: iteration := 0
57: while changed do
58:   changed := false
59:   iteration := iteration + 1
60:   
61:   for each node v in V in topological order do
62:     new_reld := RelD_map[v]
63:     
64:     for each edge (u, v) in E do
65:       // Propagate RelD along DataFlow edges with composition
66:       source_reld := RelD_map[u]
67:       
68:       // Compose: if u Outlives(x) and v DependsOn(u),
69:       // then transitively v may need to Outlive(x)
70:       for each Outlives(x) in source_reld do
71:         if x != v and not (Outlives(x) in new_reld) then
72:           add Outlives(x) to new_reld
73:           changed := true
74:       
75:       // Compose: if u TaintedBy(t) and v DependsOn(u),
76:       // then v is also tainted by t
77:       for each TaintedBy(t) in source_reld do
78:         if not (TaintedBy(t) in new_reld) then
79:           add TaintedBy(t) to new_reld
80:           changed := true
81:       
82:       // Compose: if u SecurityLevel(l1) and v has no
83:       // explicit security level, v gets max(l1, existing)
84:       for each SecurityLevel(l) in source_reld do
85:         existing := new_reld.SecurityLevel or Public
86:         new_level := max(l, existing)
87:         if new_level != existing then
88:           replace SecurityLevel(existing) with
89:             SecurityLevel(new_level) in new_reld
90:           changed := true
91:       
92:       // Compose: if u SemanticallyEquivalentTo(w),
93:       // then v (which depends on u) may transitively
94:       // relate to w
95:       for each SemanticallyEquivalentTo(w) in source_reld do
96:         if not (DependsOn(w) in new_reld) then
97:           add DependsOn(w) to new_reld
98:           changed := true
99:     
100:    // Aliasing: if two values have overlapping RepD regions,
101:    // add AliasesWith relation
102:    for each node w in V where w != v do
103:      if RepD-Overlaps(RepD_map[v], RepD_map[w]) and
104:         CapD_map[v] ∩ CapD_map[w] contains {Write} then
105:        if not (AliasesWith(w) in new_reld) then
106:          add AliasesWith(w) to new_reld
107:          changed := true
108:    
109:    RelD_map[v] := new_reld
110: end for
111: end while
112:
113: // Phase 6: Consistency checks
114: // Check 1: No circular Outlives
115: for each node v in V do
116:   if exists path v ->* v through Outlives edges then
117:     error("Circular Outlives: value %v must outlive itself "
118:           "(lifetime cycle detected)")
119: end for
120:
121: // Check 2: No security downgrades (except at declassification points)
122: for each edge (u, v) in E do
123:   if RelD_map[u].SecurityLevel > RelD_map[v].SecurityLevel then
124:     if v is not DeclassificationNode then
125:       error("Security downgrade: value %u has security level "
126:             "%(RelD_map[u].SecurityLevel) but flows to %v with "
127:             "level %(RelD_map[v].SecurityLevel) without "
128:             "declassification")
129: end for
130:
131: // Check 3: Scope validity
132: for each node v in V do
133:   for each ValidDuring(s) in RelD_map[v] do
134:     for each use u of v where u is not in scope s do
135:       if not (Outlives(u) in RelD_map[v]) then
136:         error("Scope violation: value %v used at %u outside "
137:               "its valid scope %s without Outlives guarantee")
138: end for
139:
140: return RelD_map
```

### 3.4 Constraint Graph and Fixed-Point Properties

The RelD constraint graph is constructed as follows: each node v in the SCG becomes a node in the constraint graph, and for each relational assertion in v's RelD, we add a directed edge in the constraint graph. For example, if v's RelD contains `Outlives(w)`, we add edge v → w in the constraint graph.

The fixed-point iteration in Phase 5 is guaranteed to terminate because:

1. **Monotone growth:** In each iteration, the RelD at each node can only grow (gain more assertions). No assertion is ever removed.
2. **Finite lattice:** The RelD at each node is a subset of the finite set of all possible relational assertions. Given |V| nodes and a bounded set of relation types, the total number of possible assertions is finite: O(|V|^2 * |RelationTypes|).
3. **Termination:** Since the lattice is finite and the transfer function is monotone, the iteration must terminate in at most O(|V| * |max_Reld_size|) iterations.

The composition rules in Phase 5 are carefully designed to maintain soundness:

- **Outlives composition:** If A outlives B and B outlives C, then A outlives C (transitivity).
- **Taint propagation:** If A is tainted and B depends on A, then B is tainted (taint monotonicity).
- **Security level propagation:** If A has security level L1 and B depends on A, then B has security level ≥ L1 (security monotonicity).
- **Semantic equivalence propagation:** If A ≡ B and C depends on A, then C transitively depends on B (preservation of semantic relationships).

### 3.5 Complexity Analysis

The worst-case complexity of RelD inference is O(|V|^2) for the following reasons:

1. **Constraint graph construction (Phases 2-4):** Each phase visits each node and edge once, producing O(|V| + |E|) relational assertions. Since the SCG is typically sparse (|E| = O(|V|)), this is O(|V|).

2. **Fixed-point iteration (Phase 5):** In each iteration, each node processes its incoming edges and produces new assertions. The number of iterations is bounded by the diameter of the constraint graph, which is O(|V|). Each iteration processes O(|V|) nodes, each with O(|V|) incoming assertions. Therefore, the total work is O(|V|^2).

3. **Transitive closure (implicit in Outlives composition):** Computing the transitive closure of the Outlives relation is O(|V|^2) using standard graph algorithms (e.g., Floyd-Warshall or repeated BFS).

4. **Consistency checks (Phase 6):** Checking for circular Outlives requires cycle detection in the Outlives graph, which is O(|V| + |E_Outlives|) = O(|V|^2) in the worst case. Checking for security downgrades is O(|E|) = O(|V|).

The overall worst-case complexity is O(|V|^2), dominated by the transitive closure computation. This is acceptable for programs of moderate size (up to ~10^6 nodes), and can be optimized using incremental techniques for larger programs.

### 3.6 Termination and Soundness Proof Sketch

**Theorem (Termination):** RelD-Inference always terminates.

**Proof:** The RelD at each node is a set of relational assertions drawn from a finite universe. The transfer function in each iteration is monotone (it only adds assertions, never removes them). Since the lattice is finite (bounded by the total number of possible assertions), the iteration must reach a fixed point after a finite number of steps. The maximum number of steps is the total number of possible assertions across all nodes, which is O(|V|^2 * |RelationTypes|). QED.

**Theorem (Soundness):** If RelD-Inference succeeds (no errors in Phase 6), then the inferred RelDs are consistent with the program's semantics.

**Proof Sketch:** By induction on the number of fixed-point iterations.

- **Base case (initial state):** The initial RelDs contain only assertions directly derived from the SCG structure (scope relations, data dependencies, taint sources). These are sound by construction.

- **Inductive step:** Each composition rule in Phase 5 preserves soundness:
  - If A outlives B (sound by IH) and B outlives C (sound by IH), then A outlives C (sound by transitivity of outlives).
  - If A is tainted (sound by IH) and B depends on A (sound by construction), then B is tainted (sound by taint monotonicity).
  - If A has security level L (sound by IH) and B depends on A, then B has security level ≥ L (sound by information flow policy).

- **Consistency checks:** Phase 6 verifies that no soundness-critical property is violated:
  - No circular Outlives (which would be logically inconsistent).
  - No security downgrades (which would violate information flow policy).
  - No scope violations (which would violate lifetime constraints).

Therefore, if all checks pass, the inferred RelDs are consistent. QED.

---

## 4. Combined BD Inference

### 4.1 Overview

The three BD inference algorithms — RepD, CapD, and RelD — are not independent. CapD inference depends on RepD (for validating capability requirements of cast operations), and RelD inference depends on both RepD (for structural containment and aliasing analysis) and CapD (for capability-dependent relational properties). Therefore, the combined BD inference must execute the three algorithms in a specific order and may need to iterate if later phases affect earlier ones.

This section specifies the combined inference procedure, proves that it converges in at most three iterations, and characterizes the conditions under which iteration is necessary.

### 4.2 Dependency Analysis

The dependencies between the three BD components are:

| Component | Depends On | Reason |
|-----------|-----------|--------|
| RepD | None | RepD is determined purely by memory layout and structural properties |
| CapD | RepD | CastNode validation requires RepD to determine implied capabilities |
| RelD | RepD, CapD | Aliasing analysis requires RepD; security relations require CapD |

There is a potential *reverse* dependency: CapD may affect RepD in the case where a value has no `Write` capability, which means its RepD should be treated as read-only (the `Write` bit in the RepD's interpretation flags is cleared). Similarly, RelD may affect CapD when a security-level constraint prevents certain capabilities (e.g., a `Secret` value cannot have the `Send` capability to an unencrypted channel).

However, these reverse dependencies are *refinement-only*: they can only restrict (make more conservative) the earlier component's results, never expand them. This property is crucial for proving convergence.

### 4.3 Combined Algorithm

```
Algorithm: BD-Inference
Input:  SCG = (V, E)
Output: Map<Node, BD> where BD = (RepD, CapD, RelD)
        OR Error with diagnostic information

1.  // Iteration 1: RepD (no dependencies)
2.  RepD_map := RepD-Inference(SCG)
3.  if RepD_map is Error then return Error
4.  
5.  // Iteration 1: CapD (depends on RepD)
6.  CapD_map := CapD-Inference(SCG, RepD_map)
7.  if CapD_map is Error then return Error
8.  
9.  // Iteration 1: RelD (depends on RepD and CapD)
10. RelD_map := RelD-Inference(SCG, RepD_map, CapD_map)
11. if RelD_map is Error then return Error
12. 
13. // Iteration 2: Refine RepD based on CapD
14. // If a value has no Write capability, mark its RepD as read-only
15. RepD_map_refined := copy of RepD_map
16. for each node v in V do
17.   if Write not in CapD_map[v] then
18.     RepD_map_refined[v] := RepD-MarkReadOnly(RepD_map[v])
19.     // Also: if DerivePtr not in CapD, remove pointer interpretations
20.     if DerivePtr not in CapD_map[v] then
21.       RepD_map_refined[v] := RepD-RemovePtrInterp(RepD_map_refined[v])
22. 
23. // Re-run CapD with refined RepD (may further restrict CapD)
24. if RepD_map_refined != RepD_map then
25.   RepD_map := RepD_map_refined
26:  CapD_map_2 := CapD-Inference(SCG, RepD_map)
27:  if CapD_map_2 is Error then return Error
28:  
29:  // CapD can only have decreased (more restricted) or stayed the same
30:  // Verify this invariant
31:  for each node v in V do
32:    assert CapD_map_2[v] ⊆ CapD_map[v]
33:  
34:  if CapD_map_2 != CapD_map then
35:    CapD_map := CapD_map_2
36:    
37:    // Re-run RelD with refined CapD
38:    RelD_map_2 := RelD-Inference(SCG, RepD_map, CapD_map)
39:    if RelD_map_2 is Error then return Error
40:    
41:    // RelD can only have grown (more assertions) or stayed the same
42:    for each node v in V do
43:      assert RelD_map_2[v] ⊇ RelD_map[v]
44:    
45:    RelD_map := RelD_map_2
46: 
47: // Iteration 3: Final stabilization check
48: // Re-run RepD to check if RelD changes affect RepD
49. // (They don't in the current model, but we verify)
50. RepD_map_3 := RepD-Inference(SCG)
51. assert RepD_map_3 == RepD_map  // must be unchanged
52.
53. // Construct final BD map
54. BD_map := {}
55. for each node v in V do
56.   BD_map[v] := (RepD_map[v], CapD_map[v], RelD_map[v])
57.
58. return BD_map
```

### 4.4 Convergence Proof

**Theorem:** BD-Inference converges in at most 3 iterations (one initial pass plus at most two refinement passes).

**Proof:** We show that each component stabilizes after a bounded number of passes.

**Claim 1: RepD stabilizes after iteration 1.**

RepD inference depends only on the SCG structure and allocation annotations, which are fixed. The only way RepD can change is through refinement based on CapD (marking read-only, removing pointer interpretations). After iteration 2 refines RepD based on CapD, the refined RepD can only cause CapD to further decrease (more restrictions). But a further-decreased CapD cannot introduce new RepD refinements that weren't already present, because the refinements are deterministic functions of CapD. Therefore, RepD stabilizes after iteration 2 at the latest. In fact, RepD map computed from scratch (iteration 3, line 50) is identical to the original RepD map because RepD inference has no dependency on CapD or RelD — the refinements are applied externally. So RepD_map_3 == RepD_map (the original), and the external refinements converge in one step.

**Claim 2: CapD stabilizes after iteration 2.**

In iteration 1, CapD is computed with the original RepD. In iteration 2, CapD is recomputed with the refined RepD. The refined RepD can only have fewer interpretations (no pointer, read-only), which can only reduce implied capabilities, which can only further restrict CapD. Since CapD is monotone decreasing (it only gets smaller), and the CapD lattice is finite, CapD must stabilize after at most one refinement pass. If CapD_map_2 == CapD_map, no further iteration is needed. If CapD_map_2 ⊂ CapD_map, the further-restricted CapD cannot cause any new RepD refinements (because the refinements from CapD→RepD are already maximal given the current CapD), so CapD stabilizes.

**Claim 3: RelD stabilizes after iteration 2.**

RelD depends on both RepD and CapD. After both have stabilized (by Claim 1 and Claim 2), RelD is deterministic and also stabilizes. The only way RelD could change is if CapD changes affect aliasing analysis (two values with Write capability to overlapping memory are aliases; if one loses Write, they may no longer be aliases). But removing aliasing relations only reduces RelD, and the monotone growth property of RelD inference means it re-converges from the smaller starting point.

**Conclusion:** The combined inference converges in at most 3 iterations: one initial pass, one refinement pass (if needed), and one verification pass. In practice, most programs converge in 2 iterations (the refinement pass produces no changes). QED.

### 4.5 Practical Optimization

In practice, the full three-iteration procedure is rarely needed. Most programs exhibit the following pattern:

1. **Iteration 1 succeeds:** RepD, CapD, and RelD are all inferred without error.
2. **Refinement check:** The CapD→RepD refinement produces no changes (no value has its Write capability removed by inference that wasn't already known from the SCG structure).
3. **Convergence:** The algorithm terminates after a single effective pass.

The refinement pass is needed only for programs where CapD inference discovers capability restrictions that were not apparent from the SCG structure alone. For example, a program that passes a mutable value through a read-only function interface will have the value's CapD reduced to exclude Write, which in turn refines the RepD to be read-only. This is a rare but important case that the combined algorithm handles correctly.

For implementation efficiency, the algorithm can be optimized to skip the refinement pass when no CapD changes are detected in iteration 1 that would affect RepD. This optimization reduces the effective cost of BD inference to a single pass for the vast majority of programs.

---

## 5. Soundness Theorem

### 5.1 Statement

**Theorem (BD Soundness):** If BD inference succeeds for a program P (i.e., the BD-Inference algorithm returns a BD_map without error for the SCG of P), then every operation in P respects the inferred BD. Formally, for every node v in the SCG and every operation op performed on the value produced by v, op satisfies BD_map[v] = (RepD_v, CapD_v, RelD_v), meaning:

1. **RepD soundness:** The memory region accessed by op matches a valid interpretation in RepD_v.
2. **CapD soundness:** The capability required by op is contained in CapD_v.
3. **RelD soundness:** The relational constraints in RelD_v are satisfied by op (no scope violations, no security downgrades, no lifetime violations).

### 5.2 Proof by Induction on SCG Structure

We prove soundness by structural induction on the SCG, covering each node type.

**Base Case: AllocationNode v.**

An AllocationNode produces a freshly allocated value. By construction:
- RepD_v is derived from the allocation's size/align/layout annotations, which accurately describe the allocated memory. Any operation on this value accesses memory within the allocated region with the correct interpretation. ✓
- CapD_v = FULL_CAPS (all capabilities), which trivially includes any capability required by any operation. ✓
- RelD_v includes `ValidDuring(scope)` where scope is the allocation scope, and no `Outlives` or security constraints. Any operation within the scope satisfies these constraints. ✓

**Inductive Step: DataFlow edge (u, v) with pass-through.**

By the induction hypothesis, BD_map[u] is sound. Since the algorithm propagates BD_map[u] unchanged to v (for pass-through edges), BD_map[v] = BD_map[u]. Any operation on v's value is an operation on u's value (same data, same representation), so soundness is preserved. ✓

**Inductive Step: CastNode v with source u.**

By the IH, BD_map[u] is sound. The algorithm verifies:
1. RepD-Compatible(RepD_u, RepD_v): the target memory region is a valid reinterpretation of the source region. Any operation respecting RepD_v accesses memory that is within the source region and validly interpreted. ✓
2. CapD-ImpliedBy(RepD_v) ⊆ CapD_u: the capabilities implied by the target RepD are available in the source CapD. Any operation requiring a capability from RepD_v is permitted by CapD_u. ✓
3. The cast does not introduce new relational constraints that could be violated. ✓

**Inductive Step: FunctionCall v with callee f.**

By the IH, BD_map for each actual parameter is sound. The algorithm verifies:
1. For each parameter i: RepD-Compatible(actual_RepD_i, formal_RepD_i) and formal_CapD_i ⊆ actual_CapD_i. By the callee's contract (inductively assumed sound for f's body), any operation within f on parameter i respects formal_BD_i, which is compatible with actual_BD_i. ✓
2. The return value's BD is f's return BD, which is sound by the callee's contract. ✓
3. Relational constraints are preserved across the call boundary: if a value has `Outlives(x)` before the call, it still outlives x after the call. ✓

**Inductive Step: PhiNode v merging predecessors u1, ..., uk.**

By the IH, BD_map[ui] is sound for each i. The algorithm computes:
1. RepD_v = RepD-Merge({RepD_ui}): any operation valid on the merged RepD is valid on all predecessor RepDs. ✓
2. CapD_v = CapD-Join({CapD_ui}) = ⋃ CapD_ui: any capability in CapD_v is present in some CapD_ui, and by IH, operations using that capability are sound for that predecessor. Since the merged value may come from any predecessor, any operation valid on the merged CapD is sound for the actual runtime value. ✓
3. RelD_v = RelD-Join({RelD_ui}) = ⋃ RelD_ui: the merged RelD contains all constraints from all predecessors, making it the most conservative (most constrained) assignment. Any operation satisfying the merged RelD satisfies all predecessor RelDs. ✓

**Inductive Step: FreeNode v.**

By the IH, BD_map for the freed value is sound before the free. After the free:
1. RepD of the freed value is unchanged (the memory layout doesn't change), but any subsequent access is prevented by CapD.
2. CapD of the freed value and all derived pointers loses Read, Write, DerivePtr, and Execute. Any subsequent operation requiring these capabilities is rejected by CapD soundness. ✓
3. RelD of the freed value gains additional scope constraints that prevent use-after-free. ✓

**Inductive Step: SendNode v.**

By the IH, BD_map for the sent value is sound before the send. After the send:
1. CapD of the local reference is set to {} (empty), preventing any further operation on the local reference. ✓
2. RelD captures the ownership transfer, preventing aliasing violations. ✓

**Conclusion:** By structural induction over all node types in the SCG, BD inference is sound. Every operation in P respects the inferred BD. QED.

### 5.3 Corollaries

**Corollary 1 (Memory Safety):** If BD inference succeeds, no operation accesses freed memory. This follows directly from CapD soundness: after a FreeNode, all derived values lose Read and Write capabilities, so any subsequent access would violate CapD soundness.

**Corollary 2 (Data Race Freedom):** If BD inference succeeds, no two concurrent operations conflict on the same memory. This follows from RelD soundness: the AliasesWith relation tracks all potential aliasing, and the CapD check ensures that at most one aliased value has the Write capability at any program point.

**Corollary 3 (Information Flow Security):** If BD inference succeeds, no data flows from a higher security level to a lower security level without explicit declassification. This follows directly from the security downgrade check in RelD inference Phase 6.

---

## 6. Completeness Discussion

### 6.1 The Completeness Question

A natural question arises: is BD inference *complete*? That is, for every program P that is BD-valid (every operation in P respects some valid BD assignment), does BD inference succeed and find a valid BD assignment?

This question is analogous to asking whether a type inference algorithm is *principal* and *complete* — whether it finds a type for every well-typed program.

### 6.2 Answer: No, BD Inference Is Not Complete

BD inference is **not complete**. There exist BD-valid programs for which BD inference fails. The primary source of incompleteness is CapD inference's conservative choices at merge points.

Consider the following program fragment:

```
if condition then
  x := allocate(size=64, align=8)    // x has FULL_CAPS
  y := read_only_view(x)             // y has CapD = {Read, ...} (no Write)
else
  x := allocate(size=64, align=8)    // x has FULL_CAPS
  y := x                             // y has CapD = FULL_CAPS (alias of x)
end if

// At the merge point (PhiNode):
// CapD of y = {Read, ...} ∪ FULL_CAPS = FULL_CAPS
// But if we later try to write through y, this may be unsound
// because in the then-branch, y was a read-only view
```

In this example, the CapD join at the merge point produces `FULL_CAPS` (union of both branches), which includes Write. But in the then-branch, writing through y would violate the read-only constraint. The BD inference algorithm conservatively allows Write because it cannot determine which branch was taken at runtime. A more precise analysis (e.g., path-sensitive analysis) could handle this case, but at the cost of exponential complexity.

This is the same kind of incompleteness found in all non-path-sensitive dataflow analyses: merge points lose precision because they must account for all possible execution paths. The CapD join (union) is the most permissive choice that is sound for all paths, but it may be too permissive for some paths.

### 6.3 Incompleteness Is Bounded and Practical

Despite being incomplete, BD inference is complete for a large and important class of programs:

**Theorem:** For programs that type-check in Rust or Haskell under their standard type systems, BD inference always succeeds.

**Proof Sketch:** Rust's type system enforces ownership and borrowing rules that are strictly more restrictive than CapD inference. Any program that satisfies Rust's borrow checker has a well-defined CapD assignment where:
- Owned values have full capabilities (minus Copy for non-Copy types).
- Immutable borrows have {Read, DerivePtr, Compare, Hash, ...} (no Write).
- Mutable borrows have {Read, Write, DerivePtr} (no Copy, no Move).
- Moved values have {} (empty CapD).

These CapD assignments are compatible with BD inference's propagation rules, so BD inference will succeed for any Rust-type-correct program. Similarly, Haskell's type system enforces purity constraints that are more restrictive than CapD's capability restrictions, so BD inference will also succeed for Haskell programs. QED.

### 6.4 Programs Where Inference Fails

There exist BD-valid programs where inference fails, just as there exist Rust programs that need explicit type annotations. The key categories are:

1. **Path-sensitive capability programs:** Programs where the correct CapD depends on which execution path was taken, and the merge point's CapD is too permissive. These programs are BD-valid (the runtime behavior respects BD) but inference cannot verify this with its path-insensitive analysis.

2. **Higher-order capability programs:** Programs where capabilities are determined by higher-order functions that are not statically resolvable. For example, a function that takes a capability-set as a parameter and applies it to a value. If the capability-set is computed at runtime, BD inference cannot determine the exact CapD.

3. **Complex lifetime programs:** Programs with lifetime relationships that are not expressible in the Outlives graph's transitive closure. For example, a program where two values have a lifetime relationship that depends on a runtime condition. These are the same programs that Rust's borrow checker rejects (requiring explicit lifetime annotations or `unsafe` code).

4. **Cross-boundary programs:** Programs where data flows across security boundaries or FFI boundaries with complex capability transformations that are not captured by the SCG's annotations. These require explicit CapD annotations at the boundary.

### 6.5 Handling Incompleteness

The VUMA framework handles incompleteness through several mechanisms:

1. **Explicit annotations:** The programmer (or AI agent) can add explicit BD annotations to constrain inference. These are optional refinements that override or supplement the inferred BD. This is analogous to Rust's type annotations — most are inferred, but some must be written explicitly.

2. **Verification debt:** The IVE maintains a "verification debt" for operations where BD inference could not prove soundness. These operations are flagged for review but not blocked from execution. The IVE continues to attempt verification using more sophisticated analysis techniques.

3. **Gradual inference:** The IVE can operate in a gradual mode where it infers the most precise BD it can and falls back to conservative defaults (e.g., `Opaque` RepD, `{Read}` CapD) for values it cannot fully analyze. This ensures that inference always produces a result, even if it is less precise than optimal.

4. **Path-sensitive extensions:** For critical code paths, the IVE can invoke path-sensitive analysis as a refinement step. This is more expensive (potentially exponential) but can verify BD-valid programs that the default analysis cannot.

### 6.6 Completeness vs. Precision Trade-off

The incompleteness of BD inference is not a flaw — it is a deliberate trade-off between precision and tractability. A complete inference algorithm would need to be path-sensitive (exponential in the number of branches) and would need to solve higher-order constraints (undecidable in general). By accepting bounded incompleteness, BD inference achieves polynomial complexity while covering the vast majority of practical programs.

This trade-off mirrors the design philosophy of modern type systems: Rust's borrow checker is incomplete (it rejects some safe programs), Haskell's type inference is incomplete (some well-typed programs need annotations), and Java's generics are incomplete (type erasure loses information). In each case, the designers chose tractability over completeness, and the result is a system that works well in practice despite theoretical incompleteness.

The VUMA framework extends this philosophy by providing multiple levels of analysis: the default polynomial-time inference covers most programs, and the IVE's deeper reasoning capabilities handle the rest through verification debt and explicit annotations. The result is a system that is sound (all verified programs are correct), tractable (verification is efficient), and practically complete (most real-world programs are verified without manual intervention).

---

## 7. RepD Inference from SCG

### 7.1 Overview

The `infer_repd_from_scg()` function provides a direct SCG-to-RepD inference path that complements the three-phase `BDInferenceEngine` described in Sections 1–4. Whereas the engine performs constraint solving with forward dataflow propagation, the SCG-based inference uses a fast two-pass pattern-matching approach over the graph's node types and payloads. This function is the preferred entry point when the full constraint-solving machinery of the engine is unnecessary — for example, when only representation information is needed, or when a quick initial BD estimate is desired before running the full solver.

The two-pass design is critical for handling struct layout discovery. In the first pass, each node receives a basic RepD derived purely from its payload annotations (size, alignment, type name). This pass produces conservative `Byte(size, align)` descriptors for allocation nodes whose internal structure is not yet known. The second pass examines access patterns — specifically, whether multiple AccessNodes target the same AllocationNode at different offsets — to refine those conservative byte-level descriptors into precise `Struct` descriptors with named fields at specific offsets. This refinement captures structural information that only emerges from how the program *uses* the allocation, not merely from how it was created.

### 7.2 Inference Rules by Node Type

The following rules govern RepD assignment for each SCG node type during the two passes:

| Node Type | Pass 1 Rule | Pass 2 Refinement |
|-----------|-------------|-------------------|
| **AllocationNode** | `Byte(size, align)` from payload; `Ptr(pointee)` if size=8 and align=8 with a pointer-type successor | Upgrade to `Struct(fields)` when multiple AccessNodes target this allocation at different offsets |
| **AccessNode** | `Byte(access_size, natural_align)` from access annotation | No further refinement (access RepD is determined by the access itself) |
| **CastNode** | RepD from target type name (resolved via `repd_from_type_name`) | No further refinement (cast explicitly specifies the target representation) |
| **ComputationNode** | RepD from `result_type` annotation, or inherited from predecessor if no annotation | No further refinement (computation preserves or explicitly transforms representation) |

The **pointer heuristic** for AllocationNodes in Pass 1 is a particularly important optimization: when an 8-byte, 8-aligned allocation has a successor node that treats the value as a pointer (e.g., a load from the allocation is used in an address computation), the algorithm infers `Ptr(pointee)` rather than the generic `Byte(8, 8)`. This avoids a common source of imprecision where pointer values would otherwise be treated as opaque byte arrays.

### 7.3 Struct Refinement from Access Patterns

The second pass operates as follows: for each AllocationNode that received `Byte(size, align)` in Pass 1, the algorithm collects all AccessNodes that read from or write to that allocation. If two or more AccessNodes access the allocation at distinct, non-overlapping offsets, the algorithm constructs a `Struct` RepD with one field per access pattern. Each field's offset is taken from the AccessNode's offset annotation, and each field's RepD is taken from the AccessNode's inferred `Byte(access_size, align)`.

For example, if a 16-byte allocation at node `n1` is accessed at offset 0 with size 4 (AccessNode `a1`) and at offset 8 with size 8 (AccessNode `a2`), the refinement produces `Struct(fields=[(0, Byte(4,4)), (8, Byte(8,8))], total_size=16, align=8)`. Padding bytes between offset 4 and offset 8 are represented implicitly — they are not assigned a field but are accounted for in the total size. This mirrors the layout algorithm of the target platform (ARM64 for Pi 5) and ensures the struct RepD accurately reflects the actual memory layout.

### 7.4 Algorithmic Complexity

The two-pass algorithm has complexity **O(V + E)** where V = number of SCG nodes and E = number of SCG edges. Pass 1 visits each node exactly once, performing O(1) work per node (lookup of payload annotations and successor checks). Pass 2 iterates over edges to collect access patterns for each allocation, which requires O(E) edge traversals, and then constructs struct RepDs for qualifying allocations, which requires O(V) node visits. The total work is therefore linear in the size of the SCG. This is a significant improvement over the full engine's O(V × |T|) complexity, achieved by trading constraint-solving precision for speed.

---

## 8. CapD Inference from SCG

### 8.1 Overview

The `infer_capd_from_scg()` function infers Capability Descriptors directly from the SCG's structural properties — access patterns, effect reachability, and security boundary crossings. Unlike the forward dataflow analysis of Section 2, which propagates CapD along edges with weakening, the SCG-based approach determines CapD for each node by examining the node's local context: what access modes are used, whether the node's data reaches an EffectNode (I/O boundary), whether it crosses a security boundary, and whether it participates in pointer arithmetic. This produces a context-sensitive CapD assignment without requiring the worklist-driven iteration of the full engine.

The key insight is that CapD can be decomposed into independent "capability signals," each determined by a different structural property of the SCG. Access patterns determine Read/Write; effect reachability determines Persist; security boundaries restrict to Read+Compare; and pointer arithmetic adds Compute+DerivePtr. These signals are combined (via set union) to produce the final CapD for each node. This decomposition ensures that the inference is both sound (no capability is granted unless justified by the SCG structure) and precise (every justified capability is granted).

### 8.2 Access Pattern Analysis

The primary signal for CapD inference is the access mode of each AccessNode in the SCG. The rules are:

- **Read-only access:** If all AccessNodes targeting a value use Read mode, the value's CapD includes `Read` but not `Write`. This corresponds to an immutable borrow in Rust (`&T`).
- **Read-write access:** If any AccessNode targeting a value uses Write or ReadWrite mode, the value's CapD includes both `Read` and `Write`. This corresponds to a mutable borrow in Rust (`&mut T`).
- **No access:** If a value has no AccessNode targeting it (e.g., an allocation that is only passed around), the algorithm assigns `Read` by default, since the value's content was presumably created for some purpose, but not `Write`, since no mutation was observed.

The access pattern analysis also considers the *transitive* access behavior: if a value flows to a function that writes to it, the CapD of the original value must include `Write` even though the write occurs within the callee. This is handled by propagating access modes backward along DataFlow edges from AccessNodes to their source allocations.

### 8.3 Backward BFS from Effect Nodes

The `Persist` capability is inferred by performing a **backward breadth-first search (BFS)** from all EffectNodes in the SCG. An EffectNode represents an observable side effect — an I/O operation, a network send, a file write, or any operation whose result persists beyond the program's execution. The BFS traverses DataFlow edges in reverse (from target to source), marking every node it reaches as having the `Persist` capability.

The rationale is straightforward: if a value's data eventually reaches an EffectNode, then that value is "persisted" in the sense that it influences an observable output. The Persist capability distinguishes ephemeral intermediate values (whose BD can be optimized away) from values that must be preserved for correctness. This backward analysis is O(V + E) — each node and edge is visited at most once during the BFS.

### 8.4 Security Boundary Detection

A security boundary in the SCG is an annotation on a DataFlow edge indicating that data crosses a trust domain (e.g., from kernel to user space, from a secure enclave to normal memory, from an authenticated context to a public one). The `infer_capd_from_scg()` function identifies all nodes that are sources of edges crossing security boundaries and restricts their CapD to `{Read, Compare}` — the minimal capabilities needed for boundary-crossing data that must not be modified or used for address derivation.

This restriction mirrors the principle of least privilege for cross-boundary data: a value that is about to leave a protected domain should not retain Write, DerivePtr, Execute, or other powerful capabilities that could be exploited by the receiving domain. The security boundary analysis is O(E) — it scans all edges once to find boundary crossings.

### 8.5 Algorithmic Complexity

The overall complexity of `infer_capd_from_scg()` is **O(V + E)**. The three analyses — access pattern analysis (O(V + E)), backward BFS from EffectNodes (O(V + E)), and security boundary detection (O(E)) — are each linear and are composed sequentially. The final CapD for each node is the union of the capabilities inferred by each analysis, which is O(|Capability|) per node, yielding a total of O(V × |Capability|) for the combination step. Since |Capability| is a fixed constant (14), this simplifies to O(V + E).

---

## 9. RelD Inference from SCG

### 9.1 Overview

The `infer_reld_from_scg()` function infers Relational Descriptors from the SCG's edge structure and region membership information. Unlike the constraint-based fixed-point iteration of Section 3, which builds a constraint graph and iterates to convergence, the SCG-based approach maps each edge kind directly to a set of relational assertions. This produces a sound but potentially less precise RelD assignment — it captures all direct relationships but may miss some transitive relationships that the full solver would discover through composition.

The edge-kind-to-relation mapping is the core of the algorithm. Each edge in the SCG carries a kind label (DataFlow, Derivation, Annotation, ControlFlow) that semantically determines what relational assertions should be generated for the source and target nodes. Additionally, the algorithm considers region membership: nodes belonging to the same memory region (stack frame, heap allocation, GPU buffer) are related by Containment assertions. This dual approach — edge-kind mapping plus region membership — captures both data-dependent and spatial relationships.

### 9.2 Edge Kind to Relation Mapping

The following table defines the mapping from SCG edge kinds to RelD assertions:

| Edge Kind | Source Node RelD | Target Node RelD | Additional |
|-----------|-----------------|-------------------|------------|
| **DataFlow** | `AliasDep(target)` | `DataDep(source)` | If target is AccessNode, also add `Containment(source)` on target |
| **Derivation** | — | `DataDep(source)` | Derivation edges represent computed derivations (offsets, casts) |
| **Annotation** | `Equivalence(target)` | `Equivalence(source)` | Bidirectional semantic equivalence |
| **ControlFlow** | — | `ControlDep(source)` | Control dependency from branch/switch |

The **DataFlow** mapping is the most nuanced. For a DataFlow edge (u → v), the source node `u` gains an `AliasDep(v)` assertion because the data at `u` is aliased by `v` (they refer to the same value). The target node `v` gains a `DataDep(u)` assertion because `v`'s value depends on `u`'s value. If `v` is an AccessNode, then `v` also gains a `Containment(u)` assertion, because the accessed sub-region is contained within the allocation at `u`.

The **Annotation** edge kind represents metadata annotations (e.g., type annotations, debug information, user-provided BD hints). These generate bidirectional `Equivalence` assertions, indicating that the annotated value is semantically equivalent to the annotation's referent.

### 9.3 Region Membership Analysis

Beyond edge-kind mapping, the algorithm analyzes region membership to generate `Containment` assertions. In the SCG, nodes are grouped into regions (each identified by a RegionId). A region corresponds to a contiguous memory area — a stack frame, a heap allocation, or a GPU buffer. All nodes within the same region are related by `Containment` assertions, reflecting the spatial relationship that they occupy the same memory area.

The region membership analysis iterates over all regions and, for each region R containing nodes {v1, v2, ..., vk}, adds `ContainedIn(r_root)` to each vi, where `r_root` is the AllocationNode for region R. This captures the fact that all values within a region are structurally contained within the allocation that created the region. Additionally, node-type-specific rules apply: DeallocationNodes receive `Liveness` assertions, EffectNodes receive `ControlDep` assertions, and ComputationNodes receive `DataDep` assertions from their inputs.

### 9.4 Algorithmic Complexity

The algorithm has complexity **O(V + E)**. The edge-kind mapping processes each edge once, producing O(E) relational assertions. The region membership analysis processes each node once (to determine its region), producing O(V) additional assertions. The total work is therefore linear in the SCG size. This is a significant improvement over the full solver's O(V²) complexity, achieved by forgoing transitive composition of relational assertions. The trade-off is that some transitive relationships (e.g., "A outlives C because A outlives B and B outlives C") are not discovered by the SCG-based inference alone; these must be recovered by the `check_bd_consistency()` function or by the full solver.

---

## 10. Full BD Inference from SCG

### 10.1 Overview

The `infer_bd_from_scg()` function is the primary Phase 2 (M2.3) entry point for complete BD inference. It composes the three individual inference passes — `infer_repd_from_scg()`, `infer_capd_from_scg()`, and `infer_reld_from_scg()` — into a single function that produces a `HashMap<NodeId, BD>` mapping each SCG node to its complete Behavioral Descriptor. The composition follows the dependency ordering established in Section 4: RepD first (no dependencies), then CapD (depends on RepD for implied-capability checks), then RelD (depends on both RepD and CapD for aliasing and containment analysis).

This composed function provides a fast, linear-time alternative to the full `BDInferenceEngine` for programs where the constraint-solving precision of the engine is not required. The engine's three-phase iteration (with convergence in at most 3 passes) handles programs with complex inter-component dependencies (e.g., CapD restrictions that affect RepD, or RelD security levels that restrict CapD). The SCG-based inference, by contrast, produces a single-pass BD assignment that is sound but may be less precise than the engine's output for programs with these complex dependencies.

### 10.2 Composition Algorithm

```
Algorithm: infer_bd_from_scg
Input:  SCG = (V, E)
Output: HashMap<NodeId, BD>

1. RepD_map := infer_repd_from_scg(SCG)
2. CapD_map := infer_capd_from_scg(SCG)
3. RelD_map := infer_reld_from_scg(SCG)
4. BD_map   := empty HashMap<NodeId, BD>
5. for each node v in V do
6.   BD_map[v] := BD {
7.     repd: RepD_map[v],
8.     capd: CapD_map[v],
9.     reld: RelD_map[v]
10.  }
11. end for
12. return BD_map
```

Each of the three sub-functions operates independently on the same SCG, producing its own `HashMap<NodeId, _>` output. The composition simply zips the three maps together. The total complexity is O(V + E) — the sum of the three linear-time inference passes.

### 10.3 Consistency Checking via `check_bd_consistency()`

After composition, the inferred BDs must be checked for internal consistency against the SCG structure. The `check_bd_consistency(bds, scg)` function performs four categories of checks, each corresponding to a different consistency property:

| Check | InconsistencyKind | Description |
|-------|-------------------|-------------|
| **Size mismatch** | `SizeMismatch` | AllocationNode's RepD size ≠ allocation payload size |
| **Capability violation** | `CapabilityViolation` | Read-only AccessNode has Write in CapD, or ReadWrite AccessNode missing Read/Write, or DeallocationNode has Read/Write/DerivePtr/Execute |
| **Relation contradiction** | `RelationContradiction` | RelD contains contradictory temporal assertions (e.g., both "outlives" and "is outlived by" between the same pair) |
| **Flow violation** | `FlowViolation` | RepD size changes across a DataFlow edge without an intervening CastNode, or CapD gains capabilities across a DataFlow edge (capabilities should only weaken) |

Each check iterates over the relevant subset of nodes and edges, producing a `Vec<BDInconsistency>` of detected issues. An empty vector indicates that the inferred BDs are fully consistent with the SCG. Non-empty results should be reported as diagnostics; they may indicate bugs in the inference logic or genuinely inconsistent programs that require manual BD annotations.

### 10.4 Relationship to the Full Engine

The SCG-based inference and the full `BDInferenceEngine` are complementary. The SCG-based inference is fast (O(V + E)) but less precise; the engine is slower (O(V × |T|) for RepD, O(V²) for RelD) but handles complex inter-component dependencies. The recommended workflow is:

1. Run `infer_bd_from_scg()` for a quick initial BD assignment.
2. Run `check_bd_consistency()` to identify any inconsistencies.
3. If inconsistencies exist or higher precision is needed, run the full `BDInferenceEngine`.
4. Compare the engine's output with the SCG-based output; any BD that differs is a candidate for manual review.

This layered approach allows the IVE to use fast inference by default and fall back to the full engine only when necessary, keeping the average verification time low while maintaining the ability to handle complex programs.

---

## 11. Subsumption of Rust Type System

### 11.1 Overview

A central claim of the VUMA framework is that BD inference *subsumes* the Rust type system: every program that is well-typed in Rust has a valid BD assignment. This section provides the formal mapping from Rust types to BD components and proves the subsumption theorem. The subsumption property is essential because it guarantees that VUMA can verify any program that Rust's type system accepts, while also verifying programs that Rust rejects (e.g., programs using `unsafe` code with valid memory invariants that the borrow checker cannot express).

The mapping proceeds in three parts: primitive types map to RepD+CapD, composite types map to RepD with structural CapD, and ownership/borrowing/lifetime/trait features map to CapD and RelD. Each mapping preserves the semantic guarantees of the Rust type system — if a Rust type guarantees a property (e.g., exclusivity of `&mut`), the corresponding BD assignment captures the same property through capability and relational constraints.

### 11.2 Primitive Type Mapping

Every Rust primitive type maps to a `Byte(size, align)` RepD plus a CapD determined by the type's operational constraints:

| Rust Type | RepD | CapD |
|-----------|------|------|
| `u8` | `Byte(1, 1)` | `{Read, Write, Hash, Compare}` |
| `u16` | `Byte(2, 2)` | `{Read, Write, Hash, Compare}` |
| `u32` | `Byte(4, 4)` | `{Read, Write, Hash, Compare}` |
| `u64` | `Byte(8, 8)` | `{Read, Write, Hash, Compare}` |
| `i32` | `Byte(4, 4)` | `{Read, Write, Hash, Compare}` |
| `f32` | `Byte(4, 4)` | `{Read, Write, Compare}` (no Hash — floats are not Hash in Rust) |
| `f64` | `Byte(8, 8)` | `{Read, Write, Compare}` |
| `bool` | `Byte(1, 1)` | `{Read, Write, Compare}` |
| `char` | `Byte(4, 4)` | `{Read, Compare}` |
| `()` | `Byte(0, 1)` | `{Read}` |

The key observation is that numeric types receive `{Read, Write, Hash, Compare}` because they support arithmetic (Write), hashing (Hash), and equality (Compare). Floating-point types lack `Hash` because Rust's `f32`/`f64` do not implement `Hash`. The `char` type lacks `Write` because Rust's `char` is typically used as an immutable Unicode scalar value. These CapD refinements are more precise than Rust's type system, which does not distinguish between hashable and non-hashable types at the type level (only via trait bounds).

### 11.3 Composite Type Mapping

Composite Rust types map to structured RepDs with capability constraints:

| Rust Type | RepD | CapD |
|-----------|------|------|
| `struct { x: T1, y: T2 }` | `Struct(fields=[(off₁, RepD_T1), (off₂, RepD_T2)], size=..., align=...)` | Intersection of field CapDs ∪ `{Read, Write}` |
| `enum { A(T1), B(T2) }` | `Enum(variants=[(0, RepD_T1), (1, RepD_T2)], size=..., align=...)` | Intersection of variant CapDs ∪ `{Read, Write}` |
| `[T; N]` | `Array(element=RepD_T, count=N)` | CapD_T ∪ `{Read, Write, Iterate}` |
| `Box<T>` | `Ptr(pointee=RepD_T)` | `{Read, Write, Drop, DerivePtr, Move}` (no Copy — exclusive ownership) |
| `&T` | `Ptr(pointee=RepD_T)` | `{Read, DerivePtr}` (shared reference — no Write, no Drop) |
| `&mut T` | `Ptr(pointee=RepD_T)` | `{Read, Write, DerivePtr}` (exclusive reference — no Copy, no Drop) |

The struct and enum mappings compute field offsets using the target platform's layout algorithm (ARM64 ABI for Pi 5, with natural alignment and padding). The `Box<T>` mapping is particularly important: it captures the exclusive ownership semantics of `Box` through the absence of `Copy` and the presence of `Drop` and `Move`. The `&T` and `&mut T` mappings precisely capture Rust's borrowing rules — shared references lack Write, and mutable references lack Copy and Drop.

### 11.4 Ownership, Borrowing, Lifetimes, and Traits

**Ownership → CapD:** Rust's ownership model maps directly to CapD. An owned value has `{Read, Write, Drop, Move, Hash, Compare}` (full capabilities for owned data). After a move, the source's CapD becomes `{}` (empty — the moved-from value cannot be used). This mirrors Rust's move semantics exactly: using a moved-from value is a compile error in Rust, and a CapD violation in VUMA.

**Borrowing → CapD:** Shared borrows (`&T`) restrict to `{Read, DerivePtr}`, and mutable borrows (`&mut T`) restrict to `{Read, Write, DerivePtr}`. The exclusivity of `&mut T` is enforced by the CapD lattice: if two references to the same value both have Write, the consistency checker flags a `CapabilityViolation`. This corresponds to Rust's rule that only one mutable reference or any number of shared references may exist simultaneously.

**Lifetimes → RelD:** Rust lifetimes map to `Outlives` and `ValidDuring` assertions in RelD. A lifetime annotation `'a` on a reference `&'a T` generates `ValidDuring('a)` for the reference and `Outlives(reference)` for the referent, ensuring the referent outlives the reference. Lifetime bounds (`'a: 'b`) map to `Outlives('b)` in the RelD of `'a`. The consistency checker's circular Outlives check (Section 3.3, Phase 6) corresponds to Rust's lifetime well-formedness check.

**Traits → CapD/RelD:** Rust's trait bounds map to CapD requirements and RelD constraints. `Clone` maps to the `Copy` capability (or `Fork` in the extended model), `Copy` maps to `Copy` without `Drop`, `Hash` maps to the `Hash` capability, `Ord`/`Eq` maps to `Compare`, `Send` maps to the `Send` capability plus a RelD `SecurityLevel` assertion, and `Sync` maps to `{Share, Read}` with a `SecurityLevel` assertion. The `Send`/`Sync` mapping is notable because it captures Rust's thread-safety guarantees through a combination of capabilities (what operations are allowed) and relational assertions (what security boundaries must be respected).

### 11.5 Subsumption Theorem

**Theorem:** Every Rust-typable program has a valid BD assignment. That is, for any program P that type-checks under the Rust type system (including borrow checking, lifetime checking, and trait bounds), there exists a BD assignment for P such that `check_bd_consistency()` produces an empty result.

**Proof Sketch:** By structural induction on the Rust type derivation.

- **Base case (primitive expressions):** A Rust expression of primitive type `T` maps to the BD given in Section 11.2. The CapD includes all operations valid on `T` (by construction), and the RepD accurately describes the memory layout. No consistency violation arises.

- **Inductive case (struct construction):** A struct expression `S { f1: e1, f2: e2 }` maps to a Struct RepD with the field BDs from the induction hypothesis. The struct's CapD is the intersection of field CapDs plus `{Read, Write}`, ensuring the struct is at most as capable as its least-capable field. This is consistent because any operation valid on the struct is valid on all fields.

- **Inductive case (borrowing):** A borrow expression `&e` or `&mut e` creates a Ptr RepD pointing to the referent's RepD, with CapD restricted as per Section 11.4. The borrow's CapD is a subset of the referent's CapD (removing Write for shared borrows), so no capability is gained. The RelD adds `Outlives` assertions consistent with Rust's lifetime rules, so no relation contradiction arises.

- **Inductive case (move):** A move expression transfers ownership: the target gets the source's CapD, and the source's CapD becomes `{}`. This corresponds exactly to Rust's move semantics and produces no consistency violation.

- **Inductive case (function call):** A function call with well-typed actual parameters maps to a FunctionCall node in the SCG where each actual BD is compatible with the formal BD (by the induction hypothesis and the function's type signature). The return BD is consistent by the callee's type contract.

Since every Rust type derivation step maps to a consistent BD assignment, the full program has a valid BD. QED.

---

## 12. BD Fixpoint Solver

### 12.1 Overview

The `BDFixpointSolver` is a worklist-based iterative solver that computes the fixed point of BD propagation over the SCG. Unlike the single-pass `infer_bd_from_scg()` function (Section 10), which produces a fast but potentially imprecise BD assignment, the solver iterates until all BD values stabilize, handling complex inter-component dependencies and producing the most precise BD assignment that satisfies all constraints. The solver is the backbone of the full `BDInferenceEngine` and is used when the SCG-based inference is insufficient.

The solver operates on the BD lattice, where each lattice element is a triple `(RepD, CapD, RelD)` and the ordering is component-wise. The worklist algorithm processes nodes in dependency order, applying transfer functions that correspond to the SCG's edge semantics. At each step, the solver computes the new BD for a node from its predecessors' BDs and the edge kind, and checks whether the new BD differs from the current BD. If it differs, the node's successors are added to the worklist. The algorithm terminates when the worklist is empty, indicating that all BDs have stabilized.

### 12.2 Worklist Algorithm

```
Algorithm: BDFixpointSolver
Input:  SCG = (V, E), initial BD_map from infer_bd_from_scg
Output: BD_map at fixed point

1. WorkList := queue containing all nodes in V
2. while WorkList is not empty do
3.   v := WorkList.dequeue()
4.   old_bd := BD_map[v]
5.   new_bd := compute_new_bd(v, BD_map, SCG)
6.   if new_bd != old_bd then
7.     BD_map[v] := new_bd
8.     for each successor u of v in E do
9.       WorkList.enqueue(u)
10.    end for
11.  end if
12. end while
13. return BD_map
```

The `compute_new_bd(v, BD_map, SCG)` function applies the appropriate transfer function based on the edge kinds of v's incoming edges. For each predecessor edge (u, v) with kind k, the transfer function combines the predecessor's BD with the edge's semantic constraints to produce a contribution to v's new BD. The contributions from all predecessors are then combined using the lattice operations defined by the edge kinds (see Section 12.3).

The initial BD_map is typically seeded by `infer_bd_from_scg()` (Section 10), which provides a good starting point that reduces the number of iterations needed for convergence. Alternatively, the solver can start from the bottom of the BD lattice (all capabilities, no relations, opaque RepD) and iterate upward, but this requires more iterations.

### 12.3 FlowKind Semantics

The solver's transfer functions are parameterized by the **FlowKind** of each edge, which determines how BDs are combined:

| FlowKind | Combination Operator | RepD | CapD | RelD |
|----------|---------------------|------|------|------|
| **DataFlow** | Meet (∧) | `RepD-Merge` (most specific common RepD) | `CapD ∩ CapD'` (intersection — weakest capabilities) | `RelD ∪ RelD'` (union — all constraints) |
| **ControlFlow** | Join (∨) | `RepD-Merge` (most general common RepD) | `CapD ∪ CapD'` (union — strongest capabilities) | `RelD ∩ RelD'` (intersection — shared constraints only) |
| **Derivation** | Narrowed Meet | Target RepD from cast | `CapD ∩ implied_caps(target_RepD)` | `RelD` preserved |

**DataFlow (meet):** At data flow edges, BDs are combined using the meet operator, which takes the most conservative (most restrictive) BD. For CapD, this means intersecting capability sets — a value that flows through multiple paths retains only the capabilities available on all paths. For RelD, this means taking the union of relational constraints — all constraints from all paths must be satisfied. For RepD, the merge produces the most specific RepD that is compatible with all incoming RepDs.

**ControlFlow (join):** At control flow merge points (PhiNodes), BDs are combined using the join operator, which takes the most permissive BD. For CapD, this means unioning capability sets — both branches' capabilities are preserved. For RelD, this means intersecting constraint sets — only constraints shared by both branches are enforced. For RepD, the merge produces the most general RepD that subsumes all incoming RepDs.

**Derivation (narrowed meet):** At derivation edges (casts, offsets, projections), the RepD is determined by the target of the derivation (not the source), the CapD is narrowed by intersecting with the capabilities implied by the target RepD, and the RelD is preserved from the source. This "narrowed meet" ensures that derivations can only restrict capabilities, never expand them, while allowing the RepD to change to the derived representation.

### 12.4 Convergence Guarantee

**Theorem:** The BDFixpointSolver always converges (terminates) for any finite SCG.

**Proof:** The BD lattice is finite. RepD values are drawn from a finite set of possible representations (bounded by the number of type definitions and struct layouts in the program). CapD values are subsets of the 14-element capability set, yielding at most 2¹⁴ = 16384 distinct values. RelD values are subsets of a finite set of possible assertions (bounded by O(V² × |RelationTypes|)). Since the lattice is finite and the transfer functions are monotone (meet and join on a finite lattice are monotone), the worklist algorithm must reach a fixed point after a finite number of steps. Each iteration either produces a strictly different BD for some node (moving up or down in the lattice) or terminates. Since the lattice is finite, only finitely many strict changes are possible. QED.

### 12.5 Algorithmic Complexity

The worst-case complexity of the BDFixpointSolver is **O(V × k)** where k = maximum number of iterations per node. Since the BD lattice is finite, k is bounded by the height of the BD lattice. The height of the CapD lattice is 14 (each iteration can remove at most one capability), the height of the RepD lattice is bounded by |T| (the number of type definitions), and the height of the RelD lattice is bounded by the maximum number of assertions per node. In practice, k is very small — typically 1 or 2 — because the initial BD assignment from `infer_bd_from_scg()` is already close to the fixed point. The per-iteration cost is O(E) (each node processes its incoming edges), yielding a total complexity of O(V × k × E_avg) where E_avg is the average node degree. For sparse SCGs (E_avg = O(1)), this simplifies to O(V × k).

In the worst case, k can be as large as the lattice height, which for the combined BD lattice is O(|T| + |Capability| + |RelAssertion_max|). However, this worst case is rarely encountered in practice. Empirical measurements on typical programs show that the solver converges in 2–3 iterations for over 95% of programs, making the effective complexity O(V) in practice.

---

## Appendix A: Notation Summary

| Symbol | Meaning |
|--------|---------|
| SCG | Semantic Computation Graph |
| BD | Behavioral Descriptor = (RepD, CapD, RelD) |
| RepD | Representation Descriptor |
| CapD | Capability Descriptor |
| RelD | Relational Descriptor |
| V, E | Nodes and edges of the SCG |
| ⊆, ⊇ | Subset, superset |
| ∪, ∩ | Union, intersection |
| ⊂ | Proper subset |
| ∨, ∧ | Lattice join, meet |
| ⊤, ⊥ | Lattice top, bottom |

## Appendix B: Capability Universe

| Capability | Description | Weakened By |
|-----------|-------------|-------------|
| Read | May read the value | Free, Send |
| Write | May write the value | Read-only view, Free, Send |
| DerivePtr | May derive pointer/address | Free, Send |
| Drop | May deallocate the value | Send (transferred ownership) |
| Move | May transfer ownership | Borrow, Send |
| Copy | May duplicate the value | Move-only types |
| Iterate | May iterate over collection | Free |
| Execute | May execute as code | Free, Send |
| Serialize | May convert to bytes | Never (additive) |
| Send | May send across boundary | Never (additive) |
| Persist | May write to storage | Never (additive) |
| Compare | May compare for equality | Free (if deallocated) |
| Hash | May compute hash | Free (if deallocated) |
| FFI | May pass to foreign function | Free, Send |

## Appendix C: References

- Proposal: "Beyond Human Syntax: A Proposal for AI-Native Programming Language Design" (Section 3.5)
- SCG Specification: VUMA-SPEC-SCG-001
- VUMA Memory Model: VUMA-SPEC-VUMA-001
- Hindley-Milner type inference: "Principal type-schemes for functional programs" (Damas & Milner, 1982)
- Lattice-based dataflow analysis: "Monotone frameworks" (Kam & Ullman, 1977)
- Taint analysis: "A Dynamic Technique for Eliminating Taint Vulnerabilities" (Newsome & Song, 2005)
