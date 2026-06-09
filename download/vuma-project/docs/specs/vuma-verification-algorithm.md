# VUMA Verification Algorithm Specification

**Document ID:** VUMA-SPEC-VA-001
**Author:** Agent W1-27
**Date:** 2026-03-04
**Updated:** 2026-03-06 (Wave 1 IVE capabilities)
**Status:** Draft — For Review
**Parent Specification:** Proposal "Beyond Human Syntax" Section 3.6 (VUMA Layer 6)

---

## Overview

This document specifies the verification algorithms used by the Inference and Verification Engine (IVE) to prove or refute the five global memory-safety invariants of the Verified-Unsafe Memory Access (VUMA) model. Each algorithm operates on the **Memory State Graph (MSG)** — a formal model of the program's entire memory behavior — and produces for each invariant either a proof that the invariant holds or a counterexample demonstrating a violation.

The MSG captures every allocation point, every pointer derivation chain, every deallocation point, every concurrent access with its synchronization ordering, and every cast or reinterpretation with its representation descriptor. The algorithms below describe how the IVE systematically verifies each of the five invariants — Liveness, Exclusivity, Interpretation, Origin, and Cleanup — against this graph, along with the overall pipeline that orchestrates them and the incremental verification strategy that minimizes rework on program edits.

---

## 1. Liveness Verification Algorithm

### 1.1 Purpose and Intuition

The liveness invariant asserts that every memory access targets a region that is allocated at the program point where the access occurs. This is the VUMA analogue of the "use-after-free" check, but it operates on a far richer model than the borrow checker's local lifetime analysis. Where the borrow checker reasons about lexical scopes and ownership transfers within a single function, the liveness verifier reasons about the entire program's allocation lifecycle across all execution paths. A region is "live" from its allocation point up to (but not including) its deallocation point. An access that occurs after the deallocation point on any reachable execution path constitutes a liveness violation.

### 1.2 Input

- **MSG** containing:
  - `regions`: set of all memory regions, each with `alloc_point`, `free_point` (optional), and `address_range`
  - `accesses`: set of all memory accesses, each with `target` (address expression), `program_point`, `kind` (Read/Write/Execute)
  - `derivations`: set of pointer derivations linking addresses to their source regions
  - `cfg`: control flow graph of the program for path-sensitive reasoning

### 1.3 Algorithm

```
function VERIFY_LIVENESS(msg: MSG) -> Map<Access, LivenessResult>:
    results = {}
    for each access a in msg.accesses:
        // Step 1: Determine which region the access targets
        target_region = RESOLVE_BASE_REGION(a.target, msg.derivations)
        if target_region is None:
            results[a] = LivenessResult(Violated,
                reason="Access targets unresolvable address",
                counterexample=a.target)
            continue

        // Step 2: Get allocation and deallocation points
        alloc_pp = target_region.alloc_point
        free_pp  = target_region.free_point   // None if never freed

        // Step 3: Trivial case — region never freed
        if free_pp is None:
            results[a] = LivenessResult(Proven,
                reason="Region never freed; liveness holds trivially")
            continue

        // Step 4: Path-sensitive liveness check
        if IS_REACHABLE(cfg, alloc_pp, a.program_point) and
           not IS_REACHABLE_ON_ANY_PATH_THROUGH(cfg, free_pp, a.program_point):
            // The access is reachable from alloc but NOT reachable
            // by any path that goes through free first
            results[a] = LivenessResult(Proven,
                reason="Access occurs on paths where region is still live")
        else if IS_REACHABLE_ON_ANY_PATH_THROUGH(cfg, free_pp, a.program_point):
            // There exists at least one path: alloc -> free -> access
            violating_path = FIND_PATH(cfg, alloc_pp, free_pp, a.program_point)
            results[a] = LivenessResult(Violated,
                reason="Use-after-free: access occurs after free on reachable path",
                counterexample=violating_path)
        else:
            // Access is unreachable from alloc — dead code
            results[a] = LivenessResult(Proven,
                reason="Access is unreachable from allocation; dead code")

    return results


function IS_REACHABLE_ON_ANY_PATH_THROUGH(cfg, waypoint, target):
    // Returns true if there exists any path from entry to target
    // that passes through waypoint
    return IS_REACHABLE(cfg, waypoint, target)


function RESOLVE_BASE_REGION(target, derivations):
    // Walk derivation chain until we reach a region allocation
    current = target
    while current is not a Region:
        derivation = FIND_DERIVATION(current, derivations)
        if derivation is None:
            return None   // Cannot trace back to any allocation
        current = derivation.source
    return current
```

### 1.4 Path-Sensitive Analysis

The simple reachability check above may produce false positives in cases where the deallocation is conditional. For example, if `free(r)` only occurs on error-handling paths, and an access occurs on the happy path, the access is safe even though a path from `free` to the access technically exists through an infeasible combination of branches. The IVE addresses this with **path feasibility analysis**:

```
function PATH_SENSITIVE_LIVENESS(cfg, region, access):
    // Enumerate paths from alloc_point to access.program_point
    paths = ENUMERATE_FEASIBLE_PATHS(cfg, region.alloc_point, access.program_point)

    for each path p in paths:
        if region.free_point in p.nodes:
            // This path goes through free before the access
            // Check if free_point dominates the access on this path
            free_idx = p.index_of(region.free_point)
            access_idx = p.index_of(access.program_point)
            if free_idx < access_idx:
                return LivenessResult(Violated,
                    counterexample=p)

    return LivenessResult(Proven)
```

Path enumeration is bounded to prevent exponential blowup. The IVE uses a k-limiting strategy (default k=5) on loop unrolling and a symbolic execution engine with constraint solving to prune infeasible paths. For programs where full path sensitivity is infeasible, the IVE falls back to flow-sensitive analysis and reports a **confidence level** indicating that the result is sound but imprecise.

### 1.5 Complexity

- **Worst case:** O(|accesses| × |regions|) for region resolution, multiplied by O(|paths|) for path-sensitive analysis.
- **Practical:** Most accesses resolve to a single region in O(1) via the derivation chain index. Path-sensitive analysis is bounded by k-limiting, making the practical cost O(|accesses| × k × |region_lifecycle|).
- **Optimization:** The IVE caches region liveness intervals and reuses them across accesses that target the same region, reducing the effective cost toward O(|accesses| + |regions| × |paths|).

### 1.6 Output

For each access, the algorithm produces one of:
- **Proven**: The access targets a live region on all feasible execution paths.
- **Violated**: A specific execution path leads from `free` to the access, constituting a use-after-free. The counterexample path is provided.
- **Conditional**: The access is safe under specific path conditions that the IVE can enumerate but not prove universally. A confidence score is attached.

---

## 2. Exclusivity Verification Algorithm

### 2.1 Purpose and Intuition

The exclusivity invariant asserts that no two memory accesses that conflict — i.e., at least one is a write and their target address ranges overlap — can occur simultaneously without being ordered by a synchronization edge. This is the VUMA answer to data-race freedom. Unlike Rust's borrow checker, which prevents aliasing at the structural level (no two `&mut` references to the same data), the exclusivity verifier checks the actual dynamic access patterns and their synchronization ordering. Two writes to the same address are perfectly acceptable if they are ordered by a happens-before relation — the problem only arises when two conflicting accesses might occur concurrently with no ordering constraint.

### 2.2 Input

- **MSG** containing:
  - `accesses`: set of all memory accesses with `target`, `size`, `kind` (Read/Write), and `program_point`
  - `sync_edges`: set of synchronization edges (lock acquires/releases, barrier arrivals, channel sends/receives, fork/joins)
  - `cfg`: control flow graph

### 2.3 Algorithm

```
function VERIFY_EXCLUSIVITY(msg: MSG) -> Map<(Access, Access), ExclusivityResult>:
    results = {}

    // Step 1: Build conflict pairs
    conflict_pairs = BUILD_CONFLICT_PAIRS(msg.accesses)

    // Step 2: Compute happens-before relation
    hb = COMPUTE_HAPPENS_BEFORE(msg.sync_edges, msg.cfg)

    // Step 3: Check each conflict pair
    for each (a1, a2) in conflict_pairs:
        if hb.happens_before(a1, a2) or hb.happens_before(a2, a1):
            results[(a1, a2)] = ExclusivityResult(Proven,
                reason=format("a1 and a2 ordered by happens-before"))
        else:
            // Check if they can actually execute concurrently
            if CAN_EXECUTE_CONCURRENTLY(a1, a2, msg.cfg):
                results[(a1, a2)] = ExclusivityResult(Violated,
                    reason="Data race: conflicting accesses with no synchronization",
                    counterexample=(a1, a2))
            else:
                results[(a1, a2)] = ExclusivityResult(Proven,
                    reason="Accesses are mutually exclusive on all paths")

    return results


function BUILD_CONFLICT_PAIRS(accesses):
    pairs = []
    // Optimization: group accesses by region class
    region_classes = GROUP_BY_REGION_CLASS(accesses)

    for each region_class rc in region_classes:
        class_accesses = region_classes[rc]
        for i in 0..len(class_accesses):
            for j in (i+1)..len(class_accesses):
                a1 = class_accesses[i]
                a2 = class_accesses[j]
                // At least one must be a write
                if a1.kind == Read and a2.kind == Read:
                    continue
                // Address ranges must overlap
                if RANGES_OVERLAP(a1.target, a1.size, a2.target, a2.size):
                    pairs.append((a1, a2))
    return pairs


function COMPUTE_HAPPENS_BEFORE(sync_edges, cfg):
    // Build transitive closure of synchronization ordering
    hb = HappensBeforeGraph()

    // Add intra-thread program order
    for each thread t in cfg.threads:
        for each consecutive pair (n1, n2) in t.execution_order:
            hb.add_edge(n1, n2)

    // Add inter-thread synchronization order
    for each sync_edge (src, dst, kind) in sync_edges:
        hb.add_edge(src, dst)
        // Propagate: everything before src happens-before everything after dst
        for each predecessor p of src:
            for each successor s of dst:
                hb.add_edge(p, s)

    // Compute transitive closure
    hb.compute_transitive_closure()
    return hb


function CAN_EXECUTE_CONCURRENTLY(a1, a2, cfg):
    // Two accesses can execute concurrently if they are in different threads
    // and neither happens-before the other
    if a1.thread == a2.thread:
        return false   // Same thread: ordered by program order
    return not (hb.happens_before(a1, a2) or hb.happens_before(a2, a1))
```

### 2.4 Happens-Before Computation Details

The happens-before relation is the heart of the exclusivity verification. It is computed as the transitive closure of two ordering relations:

1. **Intra-thread program order**: Within a single thread of execution, all operations are totally ordered. This ordering is extracted directly from the CFG for each thread.

2. **Inter-thread synchronization order**: Synchronization primitives establish ordering between threads. A `mutex_lock(acquire)` happens-after the corresponding `mutex_unlock(release)`. A `channel_send` happens-before the corresponding `channel_recv`. A `fork` happens-before the first operation of the child thread. A `join` happens-after the last operation of the joined thread. A `barrier` arrival happens-before all subsequent operations on any thread that passed the barrier.

The transitive closure is computed using a modified Floyd-Warshall algorithm over the set of program points, yielding an O(|nodes|^3) cost. In practice, the IVE uses a sparse representation and incremental closure computation, reducing the cost to O(|sync_edges| × |threads|).

### 2.5 Region Class Optimization

The naive algorithm considers all pairs of accesses, yielding O(|accesses|^2) conflict pair candidates. The IVE dramatically reduces this by **grouping accesses by region class** — two accesses can only conflict if they target overlapping address ranges, which requires that they target the same region (or aliasing regions). The IVE partitions accesses by their base region (computed during origin verification, Section 4), and only checks pairs within the same partition. This reduces the effective cost to O(Σ_rc |accesses_rc|^2), where the sum is over region classes. For well-structured programs where regions are small and numerous, this is close to O(|accesses|).

### 2.6 Complexity

- **Worst case:** O(|accesses|^2 × |sync_edges|) — quadratic in accesses, linear in synchronization edges.
- **With region class optimization:** O(Σ_rc |accesses_rc|^2 × |sync_edges_rc|), typically near-linear in practice.
- **Happens-before computation:** O(|program_points|^3) for full transitive closure, but sparse representations and incremental computation reduce this to O(|sync_edges| × |threads|) in practice.

### 2.7 Output

For each conflicting access pair, the algorithm produces one of:
- **Proven**: The accesses are ordered by happens-before, or they are mutually exclusive on all paths (same thread, different branches).
- **Violated**: The accesses can execute concurrently with no ordering — a data race. Both access program points and the missing synchronization are reported.
- **Conditional**: The accesses are ordered under some path conditions but not others. The IVE enumerates the conditions under which safety holds.

---

## 3. Interpretation Verification Algorithm

### 3.1 Purpose and Intuition

The interpretation invariant asserts that every memory access interprets the target bytes according to a valid Representation Descriptor (RepD). This is the VUMA replacement for type safety. In a traditional language, the type system ensures that you never read an `int` from memory that currently holds a `float` — the type of the pointer constrains the interpretation. In VUMA, there are no types; there are only RepDs, and the IVE must verify that the RepD assumed by the access matches the RepD that the memory actually has at that program point.

The key challenge is that a single region of memory may be interpreted through multiple RepDs at different times. A buffer allocated as `bytes[128]` may be written as `struct Header` at offset 0 and `float32[28]` at offset 16. The IVE must verify that each read uses a RepD that is compatible with the most recent write's RepD — or, if the memory is uninitialized, that the read's RepD is valid for uninitialized memory (typically, only `bytes[N]` or `u8[N]`).

### 3.2 Input

- **MSG** containing:
  - `accesses`: set of all accesses with `target`, `size`, `kind`, `program_point`, and `expected_RepD`
  - `derivations`: set of pointer derivations with associated RepD information
  - `region_repd_history`: for each region, a chronological log of RepD-altering operations (writes, casts, initializations)

### 3.3 Algorithm

```
function VERIFY_INTERPRETATION(msg: MSG) -> Map<Access, InterpretationResult>:
    results = {}

    for each access a in msg.accesses:
        // Step 1: Determine the target region and offset
        target_region = RESOLVE_BASE_REGION(a.target, msg.derivations)
        offset = COMPUTE_OFFSET(a.target, msg.derivations)

        // Step 2: Get the RepD at the target location at this program point
        actual_repd = GET_REPD_AT_POINT(
            target_region, offset, a.size, a.program_point, msg)

        // Step 3: Get the expected RepD of the access operation
        expected_repd = a.expected_RepD

        // Step 4: Check compatibility
        compat = CHECK_REPD_COMPATIBILITY(actual_repd, expected_repd)

        switch compat:
            case Compatible:
                results[a] = InterpretationResult(Proven,
                    reason="RepD compatible: actual matches expected")
            case Subsuming:
                // actual RepD is a superset of expected (e.g., reading i32 from bytes[4])
                results[a] = InterpretationResult(Proven,
                    reason="RepD subsuming: expected is a valid projection of actual")
            case Reinterpretation:
                // A cast is involved; check that it's a valid reinterpretation
                if IS_VALID_REINTERPRETATION(actual_repd, expected_repd, offset):
                    results[a] = InterpretationResult(Proven,
                        reason="Valid RepD reinterpretation after cast")
                else:
                    results[a] = InterpretationResult(Violated,
                        reason="Invalid cast: expected RepD not a valid reinterpretation",
                        counterexample=(actual_repd, expected_repd))
            case Incompatible:
                results[a] = InterpretationResult(Violated,
                    reason="RepD incompatible: cannot interpret actual as expected",
                    counterexample=(actual_repd, expected_repd))

        // Step 5: For reads, check initialization
        if a.kind == Read and actual_repd.includes_uninitialized:
            // Check that the specific bytes being read have been written
            // at the expected RepD before this read
            if not HAS_BEEN_WRITTEN_AS(target_region, offset, a.size,
                                       expected_repd, a.program_point, msg):
                results[a] = InterpretationResult(Violated,
                    reason="Read of uninitialized memory as non-trivial RepD",
                    counterexample=a)

    return results


function GET_REPD_AT_POINT(region, offset, size, point, msg):
    // Walk the region's RepD history backward from `point`
    // to find the most recent RepD-affecting operation
    history = msg.region_repd_history[region.id]

    // Filter to operations at or before `point` affecting [offset, offset+size)
    relevant = [op for op in history
                if op.point <= point and
                   RANGES_OVERLAP(op.offset, op.size, offset, size)]

    if relevant is empty:
        // No prior operation: memory is uninitialized
        return RepD.Uninitialized(size)

    // Merge the RepDs of the most recent writes
    // (multiple writes may cover different sub-ranges)
    return MERGE_REPDS(relevant, offset, size)


function CHECK_REPD_COMPATIBILITY(actual, expected):
    if actual == expected:
        return Compatible
    if actual.is_bytes and expected.size == actual.size:
        return Subsuming    // Reading typed data from raw bytes is OK
    if actual.is_struct and expected.is_struct and
       STRUCTURE_COMPATIBLE(actual, expected):
        return Reinterpretation
    return Incompatible


function IS_VALID_REINTERPRETATION(actual, expected, offset):
    // A reinterpretation is valid if:
    // 1. The sizes match (no truncation or extension)
    // 2. The alignment constraints of expected are satisfied at offset
    // 3. The endianness interpretation is consistent
    // 4. Any sub-field RepDs of expected are compatible with actual's sub-fields
    if expected.size > actual.available_size_at(offset):
        return false
    if offset % expected.alignment != 0:
        return false
    if expected.has_pointer_fields and not actual.was_written_as_pointer:
        return false    // Can't reinterpret integer as pointer without provenance
    return true
```

### 3.4 Initialization Tracking

A critical aspect of interpretation verification is tracking whether memory has been **initialized** at the expected RepD. Reading uninitialized bytes as raw `u8` is always safe (the bytes exist, they just have indeterminate values). But reading uninitialized bytes as a pointer, a floating-point value, or a structure with invariants is unsafe — it violates the interpretation invariant because the RepD assumes properties that the uninitialized memory does not have.

The IVE maintains a **write log** for each region: the sequence of write operations and their associated RepDs. For each read, the IVE checks whether a prior write at a compatible RepD covers the bytes being read. If the bytes were written as `u8[4]` and are being read as `i32`, the read is valid (subsuming). If the bytes were never written and are being read as `*u8` (a pointer), the read is invalid — the pointer provenance cannot be established for uninitialized memory.

```
function HAS_BEEN_WRITTEN_AS(region, offset, size, expected_repd, point, msg):
    writes = msg.region_repd_history[region.id]
    // Find all writes before `point` covering [offset, offset+size)
    covering_writes = [w for w in writes
                       if w.point < point and
                          w.kind == Write and
                          RANGES_OVERLAP(w.offset, w.size, offset, size)]

    // Check that the union of covering writes spans [offset, offset+size)
    // and each write's RepD is compatible with expected_repd
    covered = IntervalSet()
    for w in covering_writes:
        if CHECK_REPD_COMPATIBILITY(w.repd, expected_repd) != Incompatible:
            covered.add(w.offset, w.size)

    return covered.covers(offset, size)
```

### 3.5 Complexity

- **Worst case:** O(|accesses| × |RepD_checks|), where |RepD_checks| is the number of historical RepD entries that must be examined per access.
- **With indexing:** The IVE maintains a spatial index (interval tree) over each region's RepD history, reducing lookup to O(log |history|) per access. Total cost: O(|accesses| × log |max_history|).
- **Initialization check:** O(|writes_in_region|) per access in the worst case, but the interval tree reduces this to O(log |writes| + |covering_writes|).

### 3.6 Output

For each access, the algorithm produces one of:
- **Proven**: The access's expected RepD is compatible with the actual RepD at the target location, and the memory is properly initialized for the read.
- **Violated**: The RepD is incompatible (e.g., reading a pointer from integer memory), or the memory is uninitialized at the expected RepD. The actual RepD and the expected RepD are reported as the counterexample.
- **Conditional**: The RepD compatibility depends on path conditions (e.g., a cast occurs on only some paths). The IVE enumerates the conditions.

---

## 4. Origin Verification Algorithm

### 4.1 Purpose and Intuition

The origin invariant asserts that every address used in an access can be traced back through a chain of derivations to a valid region allocation. This is the VUMA answer to pointer provenance. In C, a pointer computed by casting an arbitrary integer (e.g., `(int*)0xDEADBEEF`) has no provenance — the compiler cannot verify that it points to valid memory. In Rust, such operations require `unsafe`. In VUMA, the IVE traces every address back to its origin and flags any address whose derivation chain cannot be connected to a valid allocation.

The derivation chain is a central data structure in the MSG. Every address expression is represented as a node in the derivation graph, with edges connecting derived addresses to their source addresses. The edge labels specify the derivation kind: offset (`base + N`), element access (`base[i]`), field access (`base.field`), arithmetic (`base + stride * index`), or cast (`base as T`). The origin verifier walks this graph from each access's target address back to a root node, which must be a region allocation.

### 4.2 Input

- **MSG** containing:
  - `derivations`: set of all pointer derivations, each with `source`, `operation`, `parameters`, `result`
  - `regions`: set of all region allocations

### 4.3 Algorithm

```
function VERIFY_ORIGIN(msg: MSG) -> Map<Access, OriginResult>:
    results = {}

    for each derivation d in msg.derivations:
        // Step 1: Trace derivation chain to root
        chain = TRACE_DERIVATION_CHAIN(d, msg.derivations)

        // Step 2: Check chain terminates at a valid allocation
        root = chain.last()
        if root is not a RegionAllocation:
            if root is a ConstantAddress:
                results[d] = OriginResult(Violated,
                    reason="Address derived from constant; no allocation provenance",
                    confidence=Tier1_Unsafe,
                    counterexample=chain)
            else if root is an ExternalInput:
                results[d] = OriginResult(Conditional,
                    reason="Address from external input; safety depends on runtime value",
                    confidence=Tier2_Conditional)
            continue

        // Step 3: For offset derivations, verify bounds
        target_region = root.region
        for each step in chain:
            if step.operation == Offset:
                if step.offset + step.size > target_region.size:
                    results[d] = OriginResult(Violated,
                        reason="Offset derivation exceeds source region bounds",
                        counterexample=(step, target_region.size))
                    break

            // Step 4: For arithmetic derivations, classify and verify
            else if step.operation == Arithmetic:
                pattern = CLASSIFY_ARITHMETIC(step)

                switch pattern:
                    case Linear(base, constant, index):
                        // base + constant * index: verify index bounds
                        max_index = COMPUTE_MAX_INDEX(index, msg.cfg)
                        if constant * max_index + base > target_region.size:
                            results[d] = OriginResult(Violated,
                                reason="Linear index exceeds region bounds",
                                counterexample=(step, max_index))
                        else:
                            results[d] = OriginResult(Proven,
                                reason="Linear derivation within bounds")

                    case NonLinear:
                        // Non-linear arithmetic: cannot prove bounds statically
                        results[d] = OriginResult(Conditional,
                            reason="Non-linear pointer arithmetic; cannot verify statically",
                            confidence=Tier3_RequiresRuntimeCheck)

                    case Symbolic:
                        // Symbolic expression: attempt to solve constraints
                        bounds = SOLVE_SYMBOLIC_BOUNDS(step, msg.constraints)
                        if bounds.is_within(target_region):
                            results[d] = OriginResult(Proven,
                                reason="Symbolic bounds verified within region")
                        else:
                            results[d] = OriginResult(Conditional,
                                reason="Symbolic bounds cannot be fully resolved",
                                confidence=Tier2_Conditional)

        if d not in results:
            results[d] = OriginResult(Proven,
                reason="All derivation steps within bounds")

    return results


function TRACE_DERIVATION_CHAIN(d, derivations):
    // Walk the derivation graph backward from d to the root
    chain = [d]
    current = d.source

    while current is not None and current not in RegionAllocations:
        parent = FIND_DERIVATION_BY_RESULT(current, derivations)
        if parent is None:
            chain.append(current)   // Unresolved: external or constant
            break
        chain.append(parent)
        current = parent.source

    return chain


function CLASSIFY_ARITHMETIC(step):
    // Classify the arithmetic expression into categories:
    // - Linear: base + constant * index  (verifiable)
    // - NonLinear: any expression with non-linear terms  (flagged)
    // - Symbolic: expression with symbolic variables  (constraint-solved)
    expr = step.expression

    if expr.is_linear():
        return Linear(expr.base, expr.coefficient, expr.index)
    elif expr.is_symbolic():
        return Symbolic(expr)
    else:
        return NonLinear(expr)
```

### 4.4 Tiered Confidence System

The origin verifier uses a **tiered confidence system** for derivations it cannot fully verify:

| Tier | Label | Meaning | Action |
|------|-------|---------|--------|
| Tier 1 | **Unsafe** | Address has no allocation provenance (constant address, unresolvable derivation) | Flag as violation; access is potentially unsafe |
| Tier 2 | **Conditional** | Address derivation is valid but depends on runtime values or external inputs | Flag for runtime check insertion; access is safe under stated conditions |
| Tier 3 | **RequiresRuntimeCheck** | Non-linear arithmetic; bounds cannot be proven statically | Insert runtime bounds check; access is safe if check passes |

This tiered approach ensures that the IVE never silently allows an unverified access. Every access is either proven safe, flagged as a violation, or guarded by an explicit runtime check that the IVE inserts. The programmer is informed of all Tier 2 and Tier 3 results and can choose to add stronger constraints to promote them to Tier Proven.

### 4.5 Complexity

- **Worst case:** O(|derivations| × max_chain_length), where max_chain_length is the maximum derivation chain depth.
- **Practical:** Most derivation chains are short (1–5 steps for typical pointer arithmetic). The IVE caches chain traces, so re-verification of unchanged derivations is O(1).
- **Arithmetic classification:** Linear classification is O(1). Symbolic constraint solving is O(|constraints|^2) in the worst case (SMT solver complexity), but the IVE uses incremental solving and caches results.

### 4.6 Output

For each derivation, the algorithm produces one of:
- **Proven**: The derivation chain traces to a valid allocation, and all steps are within bounds.
- **Violated**: The chain terminates at a constant address (no provenance) or an intermediate step exceeds region bounds.
- **Conditional (Tier 2)**: The chain is valid but depends on runtime conditions.
- **RequiresRuntimeCheck (Tier 3)**: Non-linear arithmetic prevents static verification; a runtime bounds check is inserted.

---

## 5. Cleanup Verification Algorithm

### 5.1 Purpose and Intuition

The cleanup invariant asserts that every allocated region is eventually freed, unless it is explicitly marked as intentionally leaked. This is the VUMA answer to memory leak detection. Unlike garbage-collected languages, which handle cleanup automatically, and unlike Rust, which enforces cleanup through the ownership model, VUMA permits manual memory management and verifies that cleanup actually occurs. The IVE does not force the programmer to free every allocation — long-lived arenas, global caches, and intentionally persistent data structures are marked as `Leaked` by annotation or inference — but it does require that every unmarked allocation has a reachable deallocation point.

The cleanup verifier also handles **double-free detection**: verifying that no region is freed more than once on any execution path. A double-free is a safety violation because the second free may corrupt the allocator's internal data structures, leading to undefined behavior.

### 5.2 Input

- **MSG** containing:
  - `regions`: set of all regions with `alloc_point`, `free_point` (optional), `leak_status` (optional)
  - `cfg`: control flow graph

### 5.3 Algorithm

```
function VERIFY_CLEANUP(msg: MSG) -> Map<Region, CleanupResult>:
    results = {}

    for each region r in msg.regions:
        // --- Leak detection ---

        if r.free_point is not None:
            // Region has an explicit deallocation point
            // Verify the free_point is reachable from alloc_point
            if IS_REACHABLE(msg.cfg, r.alloc_point, r.free_point):
                results[r] = CleanupResult(Proven,
                    reason="Region freed at reachable program point")
            else:
                results[r] = CleanupResult(Violated,
                    reason="Free point is unreachable from allocation (dead free)",
                    counterexample=FIND_UNREACHABLE_REASON(msg.cfg, r.alloc_point, r.free_point))

        else if r.leak_status == ExplicitlyLeaked:
            // Programmer or IVE has marked this region as intentionally leaked
            results[r] = CleanupResult(Proven,
                reason="Region explicitly marked as intentionally leaked")

        else if r.leak_status == InferredLeaked:
            // IVE has inferred that this region is a long-lived arena or global cache
            // Verify that the inference is sound
            if INFERENCE_IS_SOUND(r, msg):
                results[r] = CleanupResult(Proven,
                    reason="Region inferred as intentionally leaked; inference sound")
            else:
                results[r] = CleanupResult(Conditional,
                    reason="Leak inference may be unsound; review recommended")

        else:
            // Region has no free point and no leak annotation: potential leak
            results[r] = CleanupResult(Violated,
                reason="Potential memory leak: no deallocation point and no leak annotation",
                counterexample=r.alloc_point)

    // --- Double-free detection ---

    double_free_results = VERIFY_NO_DOUBLE_FREE(msg)
    results.merge(double_free_results)

    return results


function VERIFY_NO_DOUBLE_FREE(msg: MSG) -> Map<Region, CleanupResult>:
    results = {}

    for each region r in msg.regions:
        if r.free_point is None:
            continue   // No free means no double-free

        // Find all program points that free this region
        free_points = FIND_ALL_FREE_POINTS(r, msg)

        if len(free_points) > 1:
            // Multiple free points: check if any two can execute on the same path
            for each pair (fp1, fp2) in free_points:
                if EXISTS_PATH_THROUGH_BOTH(msg.cfg, fp1, fp2):
                    violating_path = FIND_PATH(msg.cfg, r.alloc_point, fp1, fp2)
                    results[r] = CleanupResult(Violated,
                        reason="Double-free: two free points reachable on same path",
                        counterexample=violating_path)
                    break
            else:
                // Multiple free points exist but are mutually exclusive
                results[r] = CleanupResult(Proven,
                    reason="Multiple free points are mutually exclusive (e.g., if-else branches)")

        // Check for re-free after free on the same path
        // This handles the case where a single free_point is executed,
        // then execution loops back and reaches it again
        if IS_IN_LOOP(msg.cfg, r.free_point):
            // The free point is inside a loop: check if the allocation
            // is also inside the same loop (each iteration allocates and frees)
            if IS_IN_SAME_LOOP(msg.cfg, r.alloc_point, r.free_point):
                results[r] = CleanupResult(Proven,
                    reason="Alloc and free in same loop iteration: no double-free")
            else:
                results[r] = CleanupResult(Violated,
                    reason="Free inside loop but alloc outside: potential double-free",
                    counterexample=DESCRIBE_LOOP_STRUCTURE(msg.cfg, r.free_point))

    return results


function FIND_ALL_FREE_POINTS(region, msg):
    // Collect all program points that deallocate this region
    // This includes explicit free() calls and implicit deallocations
    // (e.g., end of scope for stack regions, drop() calls)
    free_points = []
    for each access a in msg.accesses:
        if a.kind == Free and a.target_region == region:
            free_points.append(a.program_point)
    return free_points
```

### 5.4 Leak Inference

The IVE can automatically infer that a region should be marked as `Leaked` based on several heuristics:

1. **Global scope**: If a region is allocated at program initialization and its address is stored in a global variable, the IVE infers it is a long-lived arena.
2. **No free in any reachable code**: If the IVE can prove that no `free()` call targeting the region is reachable from any program point, it infers that the region is intentionally leaked.
3. **Arena pattern**: If a region is used as an arena (many sub-allocations, bulk deallocation at shutdown), the IVE infers the arena is intentionally leaked until the bulk free.
4. **Static lifetime**: If the region's address is captured by a closure or data structure whose lifetime is `'static` (in Rust terms) or equivalent, the IVE infers it is intentionally leaked.

These inferences are always marked as `InferredLeaked` (not `ExplicitlyLeaked`) and are subject to the soundness check in the main algorithm. If the inference is later invalidated by a program change, the IVE re-verifies the cleanup invariant for the affected region.

### 5.5 Complexity

- **Leak detection:** O(|regions| × |paths|), where |paths| is the cost of reachability analysis per region. With CFG caching, this is O(|regions| × |cfg_nodes|) in the worst case.
- **Double-free detection:** O(|regions| × |free_points_per_region|^2 × |paths|) — quadratic in the number of free points per region, multiplied by path analysis cost.
- **Practical:** Most regions have exactly one free point, reducing double-free detection to O(|regions| × |paths|). The total cost is dominated by path analysis, which is linear in CFG size per reachability query.

### 5.6 Output

For each region, the algorithm produces one of:
- **Proven**: The region has a reachable deallocation point, or is explicitly/inferrentially leaked. No double-free exists.
- **Violated**: The region has no deallocation point and no leak annotation (leak), or has multiple free points reachable on the same path (double-free). A counterexample path or allocation point is provided.
- **Conditional**: The leak inference may be unsound, requiring programmer review.

---

## 6. Overall Verification Pipeline

### 6.1 Purpose and Intuition

The five verification algorithms do not execute in isolation. They form a **pipeline** with dependencies: the output of one algorithm feeds into the input of another. The pipeline ordering is critical because later algorithms rely on results computed by earlier ones. For example, the liveness verifier needs to know which region an access targets, which is computed by the origin verifier. The exclusivity verifier needs to know which accesses are live, which is computed by the liveness verifier. The interpretation verifier needs the RepD information computed by BD inference, which runs before any VUMA verification.

### 6.2 Pipeline Definition

```
function VERIFY_PROGRAM(scg: SCG) -> VerificationReport:

    // Phase 0: Construct the Memory State Graph from the SCG
    // This is linear in SCG size: traverse all nodes and edges,
    // extract allocation/deallocation/derivation/access information
    msg = BUILD_MSG(scg)
    // Complexity: O(|scg_nodes| + |scg_edges|)

    // Phase 1: Infer Behavioral Descriptors
    // The BD inference engine computes RepD, CapD, and RelD for every
    // value in the program. This is polynomial in SCG size.
    bd_results = INFER_BEHAVIORAL_DESCRIPTORS(scg)
    // Complexity: O(|scg_nodes|^2) in worst case for constraint propagation
    // Annotate the MSG with inferred RepDs
    msg.annotate_with_bd(bd_results)

    // Phase 2: Verify Interpretation (uses BD results)
    // Check that every access uses a valid RepD for its target memory
    interpretation_results = VERIFY_INTERPRETATION(msg)
    // Complexity: O(|accesses| * |RepD_checks|)

    // Phase 3: Verify Origin (uses derivation chains)
    // Trace every address back to a valid allocation
    origin_results = VERIFY_ORIGIN(msg)
    // Complexity: O(|derivations| * max_chain_length)

    // Phase 4: Verify Liveness (uses origin results for region resolution)
    // Check that every access targets a live region
    // Origin results tell us which region each access targets
    msg.annotate_with_origin_results(origin_results)
    liveness_results = VERIFY_LIVENESS(msg)
    // Complexity: O(|accesses| * |regions|) + path analysis

    // Phase 5: Verify Exclusivity (uses liveness results)
    // Check that no two concurrent conflicting accesses exist
    // Only check accesses that are proven live (dead accesses are irrelevant)
    msg.annotate_with_liveness_results(liveness_results)
    exclusivity_results = VERIFY_EXCLUSIVITY(msg)
    // Complexity: O(|live_accesses|^2 * |sync_edges|)

    // Phase 6: Verify Cleanup (independent of other invariants)
    // Check that every region is freed or marked leaked
    cleanup_results = VERIFY_CLEANUP(msg)
    // Complexity: O(|regions| * |paths|)

    // Phase 7: Generate proofs for verified invariants
    proofs = {}
    for each (invariant, results) in [
        ("interpretation", interpretation_results),
        ("origin",         origin_results),
        ("liveness",       liveness_results),
        ("exclusivity",    exclusivity_results),
        ("cleanup",        cleanup_results)]:
        for each (entity, result) in results:
            if result.status == Proven:
                proofs[(invariant, entity)] = GENERATE_PROOF(invariant, entity, result)

    // Phase 8: Generate counterexamples for violations
    counterexamples = {}
    for each (invariant, results) in [
        ("interpretation", interpretation_results),
        ("origin",         origin_results),
        ("liveness",       liveness_results),
        ("exclusivity",    exclusivity_results),
        ("cleanup",        cleanup_results)]:
        for each (entity, result) in results:
            if result.status == Violated:
                counterexamples[(invariant, entity)] = GENERATE_COUNTEREXAMPLE(
                    invariant, entity, result)

    // Phase 9: Compute confidence levels
    confidence_map = COMPUTE_CONFIDENCE_LEVELS(
        interpretation_results, origin_results,
        liveness_results, exclusivity_results, cleanup_results)

    return VerificationReport(
        proofs=proofs,
        counterexamples=counterexamples,
        confidence=confidence_map,
        summary=SUMMARIZE_RESULTS(all_results))
```

### 6.3 Pipeline Stage Dependencies

```
SCG ──► MSG Construction
            │
            ▼
        BD Inference ──► Interpretation Verification
            │
            ▼
        Origin Verification ──► Liveness Verification
                                        │
                                        ▼
                              Exclusivity Verification
                                        │
        Cleanup Verification ◄──────────┘
            │
            ▼
        Proof / Counterexample Generation
            │
            ▼
        Confidence Level Computation
```

The dependency structure shows that:
- **Interpretation** depends only on BD inference (no other VUMA algorithm).
- **Origin** depends only on the MSG (derivation chains are self-contained).
- **Liveness** depends on origin results (needs to know which region each access targets).
- **Exclusivity** depends on liveness results (only live accesses can participate in data races).
- **Cleanup** is independent of all other invariants and can run in parallel.

This means that interpretation, origin, and cleanup can all begin as soon as the MSG is constructed and BD inference completes. Liveness must wait for origin, and exclusivity must wait for liveness. The critical path is: MSG → BD → Origin → Liveness → Exclusivity.

### 6.4 Proof Generation

For each proven invariant, the IVE generates a formal proof witness that can be independently checked. The proof format depends on the invariant:

| Invariant | Proof Format |
|-----------|-------------|
| Liveness | Path derivation showing access is between alloc and free on all feasible paths |
| Exclusivity | Happens-before chain ordering the conflicting accesses |
| Interpretation | RepD compatibility derivation and initialization log |
| Origin | Complete derivation chain from access to allocation |
| Cleanup | Reachability derivation from alloc to free, or leak annotation record |

### 6.5 Counterexample Generation

For each violated invariant, the IVE generates a concrete counterexample — a specific execution path or pair of paths that leads to the violation. Counterexamples are presented to the programmer through the projection system in human-readable form (not as raw graph paths). The counterexample includes:

1. **The violating entity**: The specific access, derivation, or region that fails the invariant.
2. **The violation path**: The execution path from program entry to the violation point.
3. **The root cause**: The specific step in the path where the invariant breaks (e.g., the free that makes the access use-after-free, the missing lock that creates a data race).
4. **Suggested fix**: A machine-generated suggestion for how to fix the violation (e.g., "add lock acquisition before this access" or "move free after last use").

### 6.6 Confidence Level Computation

Not all verification results are equally certain. The IVE assigns a confidence level to each result:

| Level | Meaning | Condition |
|-------|---------|-----------|
| **High** | Proven by exhaustive analysis | All feasible paths checked, no approximations |
| **Medium** | Proven with bounded approximation | k-limited path analysis, sound but incomplete |
| **Low** | Provisionally proven | Relies on unsolved constraints or external assumptions |
| **Unverified** | Cannot prove or disprove | IVE lacks information to determine safety |

The overall program confidence is the minimum confidence across all invariants. The IVE continuously works to raise low-confidence results toward high confidence by performing deeper analysis when resources are available.

---

## 7. Incremental Verification

### 7.1 Purpose and Intuition

Programs are not static — they evolve continuously as the programmer (or AI agent) edits the semantic model. Re-running the full verification pipeline on every edit would be prohibitively expensive. Instead, the IVE supports **incremental verification**: when a single node is added, removed, or modified in the SCG, the IVE rebuilds only the affected portion of the MSG and re-verifies only the invariants that depend on the changed subgraph.

Incremental verification is essential for the "always-compiled" model described in the proposal. The programmer should never have to wait for a full re-verification after making a change. The IVE must determine the minimal set of invariants that need re-verification, perform only those checks, and cache the results of all unaffected checks.

### 7.2 Change Impact Analysis

The first step in incremental verification is determining the **impact scope** of a change — which parts of the MSG are affected and which invariants need re-verification.

```
function ANALYZE_CHANGE_IMPACT(delta: SCGDelta, msg: MSG) -> ImpactReport:
    affected_regions = {}
    affected_derivations = {}
    affected_accesses = {}
    invariant_impacts = set()

    for each change in delta.changes:
        switch change.kind:
            case NodeAdded(node):
                // A new SCG node may introduce new allocations, accesses, or derivations
                if node.is_allocation:
                    affected_regions.add(node.region)
                    invariant_impacts.add(Cleanup)   // New region needs cleanup check
                if node.is_access:
                    affected_accesses.add(node.access)
                    invariant_impacts.add(Liveness)
                    invariant_impacts.add(Interpretation)
                    invariant_impacts.add(Exclusivity)
                if node.is_derivation:
                    affected_derivations.add(node.derivation)
                    invariant_impacts.add(Origin)

            case NodeRemoved(node):
                // Removal may invalidate previously proven invariants
                // that depended on this node's region/derivation/access
                dependents = FIND_DEPENDENTS(node, msg)
                for each dep in dependents:
                    if dep.is_region:
                        affected_regions.add(dep)
                    if dep.is_access:
                        affected_accesses.add(dep)
                    if dep.is_derivation:
                        affected_derivations.add(dep)
                invariant_impacts.add(ALL_INVARIANTS)  // Conservative: removal may affect anything

            case NodeModified(node, old_props, new_props):
                // Modification may change region bounds, access targets, derivation chains
                if old_props.region_bounds != new_props.region_bounds:
                    affected_regions.add(node.region)
                    invariant_impacts.add(Origin)   // Bounds change affects origin
                    invariant_impacts.add(Cleanup)  // Bounds change may affect cleanup
                if old_props.access_target != new_props.access_target:
                    affected_accesses.add(node.access)
                    invariant_impacts.add(Liveness)
                    invariant_impacts.add(Interpretation)
                    invariant_impacts.add(Exclusivity)
                if old_props.derivation != new_props.derivation:
                    affected_derivations.add(node.derivation)
                    invariant_impacts.add(Origin)
                    invariant_impacts.add(Liveness)   // Origin change affects liveness

            case EdgeAdded(edge) | EdgeRemoved(edge) | EdgeModified(edge):
                // Edges represent control flow or synchronization
                if edge.is_sync_edge:
                    invariant_impacts.add(Exclusivity)   // Sync change affects exclusivity
                if edge.is_control_flow:
                    // Control flow changes affect all path-dependent invariants
                    invariant_impacts.add(Liveness)
                    invariant_impacts.add(Cleanup)

    return ImpactReport(
        affected_regions=affected_regions,
        affected_derivations=affected_derivations,
        affected_accesses=affected_accesses,
        invariant_impacts=invariant_impacts)
```

### 7.3 Incremental MSG Rebuild

Once the impact scope is determined, the IVE rebuilds only the affected portion of the MSG. This is done by:

1. **Identifying the affected subgraph**: The MSG subgraph that includes all regions, derivations, and accesses in the impact report.
2. **Rebuilding the subgraph**: Re-extracting allocation, derivation, and access information from the changed SCG nodes.
3. **Splicing the subgraph**: Replacing the old subgraph in the MSG with the rebuilt version, while preserving all unaffected subgraphs and their cached verification results.

```
function INCREMENTAL_MSG_REBUILD(delta, msg, impact):
    // Step 1: Remove affected subgraph
    for each region r in impact.affected_regions:
        msg.remove_region_subgraph(r)
    for each derivation d in impact.affected_derivations:
        msg.remove_derivation_subgraph(d)
    for each access a in impact.affected_accesses:
        msg.remove_access_subgraph(a)

    // Step 2: Rebuild from changed SCG nodes
    for each change in delta.changes:
        new_subgraph = EXTRACT_MSG_SUBGRAPH(change.node, scg)
        msg.splice_subgraph(new_subgraph)

    // Step 3: Update cross-references
    // Derivations that cross region boundaries may need updating
    for each derivation d in msg.derivations:
        if d.crosses_boundary(impact.affected_regions):
            d.update_cross_references(msg)

    return msg
```

### 7.4 Incremental Re-verification

With the rebuilt MSG and the impact report, the IVE re-verifies only the affected invariants:

```
function INCREMENTAL_VERIFY(delta, msg, cached_results):
    impact = ANALYZE_CHANGE_IMPACT(delta, msg)
    msg = INCREMENTAL_MSG_REBUILD(delta, msg, impact)

    new_results = copy(cached_results)

    // Re-verify only impacted invariants, in dependency order
    if Interpretation in impact.invariant_impacts:
        new_results.interpretation = VERIFY_INTERPRETATION(msg)
            .restricted_to(impact.affected_accesses)
        // Merge: keep cached results for unaffected accesses
        new_results.interpretation = MERGE_RESULTS(
            cached_results.interpretation,
            new_results.interpretation,
            unaffected=cached_results.interpretation.keys() - impact.affected_accesses)

    if Origin in impact.invariant_impacts:
        new_results.origin = VERIFY_ORIGIN(msg)
            .restricted_to(impact.affected_derivations)
        new_results.origin = MERGE_RESULTS(
            cached_results.origin,
            new_results.origin,
            unaffected=cached_results.origin.keys() - impact.affected_derivations)

    if Liveness in impact.invariant_impacts:
        // Liveness depends on origin; if origin changed, re-verify liveness
        // for accesses whose region resolution may have changed
        live_accesses = impact.affected_accesses
        if Origin in impact.invariant_impacts:
            live_accesses += FIND_ACCESSES_DEPENDENT_ON(impact.affected_derivations, msg)
        new_results.liveness = VERIFY_LIVENESS(msg)
            .restricted_to(live_accesses)
        new_results.liveness = MERGE_RESULTS(
            cached_results.liveness,
            new_results.liveness,
            unaffected=cached_results.liveness.keys() - live_accesses)

    if Exclusivity in impact.invariant_impacts:
        // Exclusivity depends on liveness; if liveness changed, re-verify
        // exclusivity for conflict pairs involving changed accesses
        live_changed = impact.affected_accesses
        if Liveness in impact.invariant_impacts:
            live_changed += FIND_ACCESSES_WITH_CHANGED_LIVENESS(new_results, cached_results)
        conflict_pairs = FIND_CONFLICT_PAIRS_INVOLVING(live_changed, msg)
        new_results.exclusivity = VERIFY_EXCLUSIVITY(msg)
            .restricted_to(conflict_pairs)
        new_results.exclusivity = MERGE_RESULTS(
            cached_results.exclusivity,
            new_results.exclusivity,
            unaffected=cached_results.exclusivity.keys() - conflict_pairs)

    if Cleanup in impact.invariant_impacts:
        new_results.cleanup = VERIFY_CLEANUP(msg)
            .restricted_to(impact.affected_regions)
        new_results.cleanup = MERGE_RESULTS(
            cached_results.cleanup,
            new_results.cleanup,
            unaffected=cached_results.cleanup.keys() - impact.affected_regions)

    return new_results
```

### 7.5 Caching Strategy

The IVE maintains a **verification cache** that stores the results of all invariant checks keyed by the MSG subgraph they depend on. When a change occurs:

1. **Cache invalidation**: Results that depend on the changed subgraph are invalidated. The IVE uses a fine-grained dependency map (each result records which MSG elements it depends on) to minimize invalidation.
2. **Cache reuse**: Results that do not depend on the changed subgraph are reused without re-computation. This is the primary source of incremental speedup.
3. **Cache compaction**: Over time, the cache may accumulate results for subgraphs that are no longer part of the MSG (e.g., after a deletion). The IVE periodically compacts the cache by removing entries whose key subgraphs are no longer present.

The cache is implemented as a content-addressed store: the key is a hash of the MSG subgraph, and the value is the verification result. This allows the IVE to detect when a rebuilt subgraph is identical to a previously verified one (e.g., after an undo operation) and reuse the cached result directly.

### 7.6 Complexity of Incremental Update

- **Change impact analysis:** O(|delta| × |dependency_map_lookup|) — linear in the size of the change, with constant-time dependency lookups.
- **MSG rebuild:** O(|affected_subgraph|) — proportional to the size of the changed subgraph, not the entire MSG.
- **Re-verification:** O(|affected_entities| × verification_cost_per_entity) — only the affected accesses, derivations, and regions are re-verified.
- **Overall:** O(delta_size × log(program_size)) — the log factor comes from dependency map lookups and cache indexing. This is exponentially faster than full re-verification, which is O(program_size).

In practice, for a single-line edit that affects one region and a handful of accesses, the incremental verification completes in milliseconds, enabling the "always-verified" experience described in the proposal.

---

## 8. Multi-Pointer Aliasing Analysis

### 8.1 Purpose and Intuition

The basic exclusivity verifier (Section 2) assumes that each pointer targets a single, well-defined region. However, real programs — especially those using `unsafe` code, FFI boundaries, or arena allocators — frequently create situations where multiple pointers may alias the same memory. The multi-pointer aliasing analysis extends the IVE's ability to reason about these cases by computing **alias sets**: groups of pointers that may (or must) refer to overlapping address ranges at a given program point. Without this analysis, the IVE would either miss subtle exclusivity violations caused by aliasing or produce false positives by treating all pointers to the same region as potentially aliasing when only a subset actually do.

The core insight is that aliasing is not a binary property. Two pointers may *must-alias* (they always point to the same address), *may-alias* (they might point to the same address on some execution path), or *no-alias* (they never point to the same address). The IVE must distinguish these cases to produce precise exclusivity results. Must-alias pairs are always conflicting; may-alias pairs require further path-sensitive analysis; no-alias pairs are safe.

### 8.2 Data Structures

**`DerivationAliasInfo`** captures the aliasing relationship between two pointer derivations:

```
struct DerivationAliasInfo:
    derivation_a: DerivationId
    derivation_b: DerivationId
    alias_kind: AliasKind           // MustAlias, MayAlias, NoAlias
    overlap_range: Option<(Address, Address)>  // Concrete overlap if MustAlias
    confidence: f64                 // 0.0–1.0 for MayAlias, 1.0 for MustAlias/NoAlias
    program_points: Set<ProgramPoint>  // Where this alias relationship holds
```

**Alias sets** are represented using a union-find (disjoint set) data structure where each derivation starts in its own set, and sets are merged when two derivations are determined to may-alias. Each set has a representative element and tracks all derivations in the set along with their overlapping address ranges.

### 8.3 Algorithm

```
function COMPUTE_ALIAS_SETS(msg: MSG) -> Vec<AliasSet>:
    uf = UnionFind(msg.derivations.len())

    // Phase 1: Identical derivation chains → MustAlias
    chains = {}
    for each derivation d in msg.derivations:
        chain_key = CANONICALIZE(TRACE_DERIVATION_CHAIN(d, msg.derivations))
        if chain_key in chains:
            uf.union(d.id, chains[chain_key].id)
        else:
            chains[chain_key] = d

    // Phase 2: Same base region + overlapping offsets → MayAlias
    region_groups = GROUP_BY_BASE_REGION(msg.derivations)
    for each (region, derivs) in region_groups:
        for i in 0..len(derivs):
            for j in (i+1)..len(derivs):
                d1 = derivs[i]
                d2 = derivs[j]
                if RANGES_MAY_OVERLAP(d1.computed_range, d2.computed_range):
                    uf.union(d1.id, d2.id)

    // Phase 3: Collect alias sets from union-find
    sets = COLLECT_SETS(uf)
    return sets


function VERIFY_MULTI_POINTER_EXCLUSIVITY(msg: MSG, alias_sets: Vec<AliasSet>) ->
        Map<(DerivationId, DerivationId), ExclusivityResult>:
    results = {}

    for each alias_set as in alias_sets:
        // Get all accesses through pointers in this alias set
        accesses = [a for a in msg.accesses
                    if a.derivation in as.members]
        // Check exclusivity within the alias set
        conflict_pairs = BUILD_CONFLICT_PAIRS(accesses)
        for each (a1, a2) in conflict_pairs:
            alias_info = DerivationAliasInfo(
                derivation_a=a1.derivation,
                derivation_b=a2.derivation,
                alias_kind=DETERMINE_ALIAS_KIND(a1, a2, as),
                overlap_range=COMPUTE_OVERLAP(a1, a2),
                confidence=COMPUTE_CONFIDENCE(a1, a2, as),
                program_points=INTERSECT_POINTS(a1, a2))
            result = CHECK_EXCLUSIVITY_WITH_ALIAS(a1, a2, alias_info, msg)
            results[(a1.derivation, a2.derivation)] = result

    return results
```

### 8.4 Union-Find Algorithm Details

The union-find data structure uses **path compression** and **union by rank** to achieve near-constant amortized time per operation. The `find(x)` operation returns the representative of the set containing `x`, compressing the path to the root on each lookup. The `union(x, y)` operation merges the sets containing `x` and `y`, attaching the smaller-rank tree under the larger-rank root. This guarantees that the tree depth is O(α(n)), where α is the inverse Ackermann function, which is ≤ 4 for all practical input sizes. The total cost of computing alias sets over |D| derivations is O(|D| × α(|D|)) for the union-find operations, plus O(|D|^2) for the pairwise comparison within same-region groups (optimized to near-linear by the region-grouping heuristic).

### 8.5 Complexity

- **Union-find construction:** O(|D| × α(|D|)) amortized, where |D| is the number of derivations.
- **Same-region pairwise comparison:** O(Σ_r |D_r|^2), where D_r is the set of derivations targeting region r. Typically near-linear.
- **Exclusivity checking within alias sets:** O(Σ_as |A_as|^2 × |S_as|), where A_as is the set of accesses through the alias set and S_as is the synchronization edges relevant to those accesses.
- **Overall:** Dominated by the pairwise comparison, typically O(|D|^2) in the worst case but O(|D|) for well-partitioned programs where each region has a small number of derivations.

### 8.6 Output

For each pair of potentially aliasing derivations, the algorithm produces one of:
- **Proven**: The derivations are provably non-aliasing (NoAlias), or they alias but all conflicting accesses are properly ordered.
- **Violated**: The derivations must-alias and conflicting accesses can execute concurrently — a definite data race via aliasing.
- **Conditional (MayAlias)**: The derivations may alias on some paths; the IVE enumerates the conditions under which the aliasing leads to a conflict.

---

## 9. Interval Tree Optimization

### 9.1 Purpose and Intuition

The interpretation verifier (Section 3) and the overlap detection in conflict pair construction (Section 2) both require efficient queries of the form: "which intervals overlap with this query interval?" The naive approach iterates all intervals for each query, yielding O(n × m) total cost for n queries against m intervals. For large programs with thousands of regions and millions of accesses, this quadratic cost is prohibitive. The `AccessIntervalTree` provides an O(n log n) construction and O(log n + k) query time (where k is the number of results), dramatically accelerating overlap detection across the IVE.

The interval tree is an augmented balanced binary search tree (typically a red-black tree or AVL tree) where each node stores an interval [lo, hi) and the maximum hi value among all intervals in the node's subtree. This augmentation enables efficient pruning: if the query interval's low bound exceeds the maximum hi in a subtree, no interval in that subtree can overlap the query, and the entire subtree can be skipped.

### 9.2 Data Structure

```
struct AccessIntervalTree:
    root: Option<IntervalNode>
    size: usize                    // Number of intervals stored

struct IntervalNode:
    interval: (Address, Address)   // [lo, hi) — half-open interval
    max_hi: Address                // Maximum hi in this node's subtree
    left: Option<Box<IntervalNode>>
    right: Option<Box<IntervalNode>>
    height: u32                    // For AVL balancing
    data: IntervalData             // Associated access/RepD information

struct IntervalData:
    access_id: Option<AccessId>    // If this interval represents an access
    repd: Option<RepD>             // If this interval represents a RepD range
    region_id: Option<RegionId>    // If this interval represents a region range
    program_point: ProgramPoint    // Where this interval is active
```

### 9.3 Construction Algorithm

```
function BUILD_INTERVAL_TREE(intervals: Vec<(Address, Address, IntervalData)>) -> AccessIntervalTree:
    // Sort intervals by low endpoint for balanced construction
    sorted = intervals.sorted_by(|a, b| a.0.cmp(&b.0))
    return BUILD_BALANCED(sorted, 0, len(sorted))


function BUILD_BALANCED(sorted, lo, hi) -> IntervalNode:
    if lo >= hi:
        return None
    mid = (lo + hi) / 2
    node = IntervalNode(interval=sorted[mid].0..sorted[mid].1, data=sorted[mid].2)
    node.left = BUILD_BALANCED(sorted, lo, mid)
    node.right = BUILD_BALANCED(sorted, mid + 1, hi)
    // Compute max_hi augmentation
    node.max_hi = max(node.interval.hi,
                      max(node.left.max_hi_or(MIN_ADDR),
                          node.right.max_hi_or(MIN_ADDR)))
    // AVL balance
    node.height = 1 + max(node.left.height_or(0), node.right.height_or(0))
    return BALANCE(node)
```

### 9.4 Query Algorithm

```
function QUERY_OVERLAPS(tree: AccessIntervalTree, query: (Address, Address)) -> Vec<IntervalData>:
    results = []
    QUERY_NODE(tree.root, query, &mut results)
    return results


function QUERY_NODE(node: Option<IntervalNode>, query: (Address, Address), results: &mut Vec<IntervalData>):
    if node is None:
        return

    // Prune: if query is entirely above this subtree's max, skip
    if query.0 >= node.max_hi:
        return

    // Recurse left (may overlap)
    QUERY_NODE(node.left, query, results)

    // Check current node
    if INTERVALS_OVERLAP(node.interval, query):
        results.push(node.data)

    // Recurse right only if query could overlap right subtree
    if query.1 > node.interval.0:
        QUERY_NODE(node.right, query, results)
```

### 9.5 Application to IVE Phases

The interval tree is used in three IVE phases:

1. **Conflict pair construction (Section 2):** Build an interval tree over all access address ranges. For each write access, query overlapping accesses. This reduces the O(|A|^2) pairwise check to O(|A| × log |A| × k), where k is the average number of overlapping accesses per write.

2. **RepD history lookup (Section 3):** Build an interval tree over each region's RepD history entries. For each access, query the interval tree to find the most recent RepD entries covering the access range. This reduces O(|A| × |H|) to O(|A| × (log |H| + k)).

3. **Alias overlap detection (Section 8):** Build an interval tree over derivation ranges within each region. For each derivation, query overlapping derivations to identify may-alias pairs. This reduces O(|D_r|^2) per region to O(|D_r| × log |D_r| × k).

### 9.6 Complexity

- **Construction:** O(n log n) for n intervals, using sorted median construction with AVL balancing.
- **Insertion:** O(log n) amortized (with rebalancing).
- **Query:** O(log n + k) per query, where k is the number of overlapping results.
- **Total for conflict pairs:** O(|A| log |A| + |A| × k_avg), where k_avg is the average number of conflicts per access. For sparse conflicts (k_avg ≈ O(1)), this is O(|A| log |A|).
- **Space:** O(n) for n intervals.

---

## 10. Deep Type Confusion Detection

### 10.1 Purpose and Intuition

The interpretation verifier (Section 3) checks that each access uses a RepD compatible with the memory's current state. However, a deeper class of bugs exists that the basic RepD compatibility check misses: **type confusion** where the high-level structure of the data is violated even though the low-level byte layout is compatible. For example, reading a union through the wrong discriminator, or accessing an enum after it has been written with a different variant, produces semantically incorrect results even though the RepD byte sizes and alignments match. The deep type confusion detector extends interpretation verification to track the semantic structure of unions, enums, and discriminated types at the level of discriminators and variant tags, not just byte layouts.

The key insight is that type confusion is a *temporal* property: it depends on the sequence of writes that have occurred at a given memory location. A union is safe to read as variant A only if the most recent write (or initialization) was through variant A. Similarly, an enum value is safe to match on variant B only if the discriminator indicates variant B. The IVE must track these discriminator values across the program's execution to verify that no read uses the wrong variant.

### 10.2 Data Structures

**`DeepConfusionKind`** enumerates the categories of deep type confusion the IVE detects:

```
enum DeepConfusionKind:
    UnionDiscriminatorMismatch:
        expected_discriminator: DiscriminatorValue
        actual_discriminator: DiscriminatorValue
        union_type: RepD

    EnumVariantMismatch:
        expected_variant: VariantId
        actual_variant: VariantId
        enum_type: RepD

    DiscriminatorCorrupted:
        expected_range: (DiscriminatorValue, DiscriminatorValue)
        actual_value: DiscriminatorValue
        reason: String

    CrossVariantFieldAccess:
        access_variant: VariantId
        active_variant: VariantId
        field_offset: usize
        field_repd: RepD
```

**Union discriminator tracking** maintains a map from each union-typed memory location to its current discriminator state:

```
struct UnionDiscriminatorState:
    location: (RegionId, usize)         // Region + offset
    current_discriminator: Option<DiscriminatorValue>
    discriminator_type: RepD            // Typically u8, u16, or i32
    variant_map: Map<DiscriminatorValue, VariantId>
    write_history: Vec<(ProgramPoint, DiscriminatorValue, VariantId)>
```

**Enum variant tracking** extends this to Rust-style enums where the discriminator is implicit:

```
struct EnumVariantState:
    location: (RegionId, usize)
    active_variant: Option<VariantId>
    variant_repds: Map<VariantId, RepD>
    transition_history: Vec<(ProgramPoint, VariantId, VariantId)>  // (point, from, to)
```

### 10.3 Algorithm

```
function DETECT_DEEP_TYPE_CONFUSION(msg: MSG) -> Vec<DeepConfusionReport>:
    reports = []

    // Phase 1: Build discriminator/variant state machines
    union_states = BUILD_UNION_DISCRIMINATOR_STATES(msg)
    enum_states = BUILD_ENUM_VARIANT_STATES(msg)

    // Phase 2: Walk writes to update discriminator states
    for each write w in msg.writes_ordered_by_program_point():
        if w.target_repd.is_union:
            state = union_states[w.target_location]
            new_disc = INFER_DISCRIMINATOR_FROM_WRITE(w)
            state.current_discriminator = new_disc
            state.write_history.append((w.point, new_disc, DISC_TO_VARIANT(new_disc)))

        if w.target_repd.is_enum:
            state = enum_states[w.target_location]
            new_variant = INFER_VARIANT_FROM_WRITE(w)
            state.active_variant = new_variant
            state.transition_history.append((w.point, state.active_variant, new_variant))

    // Phase 3: Check each read for confusion
    for each read r in msg.reads_ordered_by_program_point():
        if r.target_repd.is_union:
            state = union_states[r.target_location]
            expected_variant = INFER_VARIANT_FROM_READ(r)
            if state.current_discriminator is Some(disc):
                active_variant = DISC_TO_VARIANT(disc)
                if active_variant != expected_variant:
                    reports.append(DeepConfusionReport(
                        kind=UnionDiscriminatorMismatch(disc, expected_variant.disc, r.target_repd),
                        location=r.target_location,
                        access_point=r.program_point,
                        severity=DETERMINE_SEVERITY(active_variant, expected_variant)))

        if r.target_repd.is_enum:
            state = enum_states[r.target_location]
            read_variant = INFER_VARIANT_FROM_READ(r)
            if state.active_variant is Some(active):
                if active != read_variant:
                    reports.append(DeepConfusionReport(
                        kind=EnumVariantMismatch(active, read_variant, r.target_repd),
                        location=r.target_location,
                        access_point=r.program_point,
                        severity=DETERMINE_SEVERITY(active, read_variant)))

    // Phase 4: Cross-variant field access analysis
    for each access a in msg.accesses:
        if a.target_repd.is_enum or a.target_repd.is_union:
            state = GET_STATE(a, union_states, enum_states)
            access_variant = INFER_ACCESS_VARIANT(a)
            active_variant = state.current_variant_at(a.program_point)
            if access_variant != active_variant:
                field_offset = COMPUTE_FIELD_OFFSET(a)
                if NOT_VALID_IN_VARIANT(field_offset, active_variant):
                    reports.append(DeepConfusionReport(
                        kind=CrossVariantFieldAccess(access_variant, active_variant,
                                                     field_offset, a.target_repd),
                        location=a.target_location,
                        access_point=a.program_point,
                        severity=High))

    return reports
```

### 10.4 Severity Classification

Not all type confusion is equally dangerous. The IVE classifies confusion severity as:

| Severity | Condition | Example |
|----------|-----------|---------|
| **Critical** | Reading a pointer field from the wrong variant | Union has `{ptr, i32}`; reading `ptr` when `i32` is active → use of garbage pointer |
| **High** | Reading a non-pointer field from wrong variant with different size | Reading 8-byte variant as 4-byte → potential buffer over-read |
| **Medium** | Same-size variant confusion | Two variants both 4 bytes, different interpretation |
| **Low** | Discriminator range violation | Discriminator value outside expected range but not yet read |

### 10.5 Complexity

- **Discriminator state construction:** O(|writes_to_union_or_enum|), linear in the number of relevant writes.
- **Discriminator inference per write:** O(1) amortized with field offset tracking.
- **Read checking:** O(|reads_from_union_or_enum|), linear in the number of relevant reads.
- **Cross-variant field analysis:** O(|accesses_to_tagged_types| × |variants_per_type|), typically O(|A_tagged|) since most tagged types have few variants.
- **Overall:** O(|A_tagged| + |W_tagged|), linear in the number of accesses and writes to tagged types. This is efficient because only a subset of all memory operations involve unions or enums.

### 10.6 Output

For each detected confusion, the algorithm produces a `DeepConfusionReport` containing:
- The confusion kind with expected vs. actual discriminator/variant.
- The memory location where the confusion occurs.
- The program point of the confusing access.
- The severity classification.
- A suggested fix (e.g., "add discriminator check before this access" or "match on variant before reading field").

---

## 11. Concurrent Exclusivity Verification

### 11.1 Purpose and Intuition

The basic exclusivity verifier (Section 2) checks for data races by computing a happens-before relation and checking that all conflicting access pairs are ordered. However, the basic algorithm has two limitations: (1) it uses a coarse-grained happens-before computation that may miss ordering opportunities from complex synchronization patterns (e.g., transitive ordering through multiple locks, seqlock patterns, RCU read-side critical sections); and (2) it does not detect **deadlocks** — situations where two or more threads are blocked waiting for each other, preventing forward progress. The `ConcurrentExclusivityVerifier` addresses both limitations by constructing a richer `HappensBeforeGraph` that captures partial orders more precisely and by performing deadlock cycle detection on the lock acquisition graph.

The concurrent exclusivity verifier is designed for programs that use shared-memory concurrency with explicit synchronization (mutexes, read-write locks, condition variables, barriers, seqlocks, RCU). It does not replace the basic exclusivity verifier; rather, it extends it with additional analysis passes that run when the MSG contains concurrent access patterns. Single-threaded programs or programs with only simple fork-join parallelism can rely on the basic verifier alone.

### 11.2 Data Structures

**`HappensBeforeGraph`** extends the basic happens-before relation with fine-grained edge types and incrementally computed transitive closure:

```
struct HappensBeforeGraph:
    edges: Map<(ProgramPoint, ProgramPoint), HBEdgeKind>
    closure: SparseBitMatrix         // Incremental transitive closure
    lock_graph: LockAcquisitionGraph // For deadlock detection

enum HBEdgeKind:
    ProgramOrder                    // Intra-thread sequencing
    LockReleaseAcquire(mutex_id)    // unlock→lock on same mutex
    RWLockReleaseAcquire(rwlock_id, mode)  // unlock→lock, with Read/Write mode
    BarrierSync(barrier_id)         // Barrier arrival → all subsequent ops
    ChannelSendRecv(channel_id)     // send → recv
    ForkJoin(thread_id)             // fork → child start, child end → join
    SeqLockWriteRead(seqlock_id)    // seqlock write → read with version check
    RCUSync(rcu_id)                 // RCU grace period → reclamation
```

**`LockAcquisitionGraph`** tracks lock acquisition order for deadlock detection:

```
struct LockAcquisitionGraph:
    nodes: Set<LockId>
    edges: Map<(LockId, LockId), Vec<AcquisitionRecord>>  // Lock A held → Lock B acquired

struct AcquisitionRecord:
    thread: ThreadId
    held_lock: LockId
    acquired_lock: LockId
    program_point: ProgramPoint
```

### 11.3 Data Race Detection Algorithm

```
function DETECT_DATA_RACES(msg: MSG, hb: HappensBeforeGraph) -> Vec<DataRaceReport>:
    races = []
    conflict_pairs = BUILD_CONFLICT_PAIRS_INTERVAL_TREE(msg.accesses)

    for each (a1, a2) in conflict_pairs:
        if a1.thread == a2.thread:
            continue  // Same thread: ordered by program order

        // Check happens-before with incremental closure
        if hb.happens_before(a1.program_point, a2.program_point) or
           hb.happens_before(a2.program_point, a1.program_point):
            continue  // Ordered: no data race

        // Check lock-based mutual exclusion
        if SHARES_LOCK(a1, a2, msg):
            continue  // Both under same lock: mutually exclusive

        // Check RWLock read-read exclusion
        if IS_READ_READ_UNDER_RWLOCK(a1, a2, msg):
            continue  // Concurrent reads under read-lock: safe

        // Data race confirmed
        races.append(DataRaceReport(
            access_a=a1, access_b=a2,
            missing_sync=SUGGEST_SYNC(a1, a2),
            severity=if a1.kind == Write or a2.kind == Write then WriteRace else ReadRace))

    return races
```

### 11.4 Deadlock Detection Algorithm

```
function DETECT_DEADLOCKS(hb: HappensBeforeGraph) -> Vec<DeadlockReport>:
    deadlocks = []

    // Build lock acquisition graph from HB edges
    lag = hb.lock_graph

    // Find cycles in the lock acquisition graph
    // A cycle A→B→C→A means: some thread holds A and acquires B,
    // some thread holds B and acquires C, some thread holds C and acquires A
    cycles = FIND_CYCLES(lag)

    for each cycle in cycles:
        // A potential deadlock: report it
        deadlocks.append(DeadlockReport(
            lock_cycle=[lock for lock in cycle],
            acquisition_chains=[FIND_ACQUISITION_CHAIN(lag, cycle_edge)
                                for cycle_edge in cycle.edges()],
            severity=DETERMINE_DEADLOCK_SEVERITY(cycle)))

    return deadlocks


function FIND_CYCLES(lag: LockAcquisitionGraph) -> Vec<Vec<LockId>>:
    // Use DFS-based cycle detection with coloring
    // White = unvisited, Gray = in-progress, Black = done
    cycles = []
    color = Map<LockId, Color>()

    for each lock in lag.nodes:
        color[lock] = White

    for each lock in lag.nodes:
        if color[lock] == White:
            DFS_CYCLE(lock, lag, color, [], &mut cycles)

    return cycles


function DFS_CYCLE(node, lag, color, path, cycles):
    color[node] = Gray
    path.append(node)

    for each (successor, records) in lag.edges_from(node):
        if color[successor] == Gray:
            // Found a cycle: extract it from the path
            cycle_start = path.index_of(successor)
            cycle = path[cycle_start..]
            cycles.append(cycle)
        elif color[successor] == White:
            DFS_CYCLE(successor, lag, color, path, cycles)

    path.pop()
    color[node] = Black
```

### 11.5 Complexity

- **Happens-before graph construction:** O(|sync_edges| × |threads|) for the sparse transitive closure. The incremental closure avoids full recomputation when new edges are added.
- **Data race detection:** O(|conflict_pairs| × |hb_lookup|), where hb_lookup is O(1) with the sparse bit matrix. Total: O(|A|^2 / |regions|) with interval tree optimization.
- **Deadlock detection:** O(|locks| + |acquisition_edges|) for DFS-based cycle detection — linear in the size of the lock graph.
- **Lock acquisition graph construction:** O(|sync_edges|) — each lock acquire/release adds one edge.
- **Overall:** Dominated by data race detection at O(|A|^2 / |regions|), with deadlock detection adding only O(|L| + |E_L|) where L is locks and E_L is lock acquisition edges.

### 11.6 Output

For each detected data race, the algorithm produces a `DataRaceReport` with the two conflicting accesses, the missing synchronization primitive, and a severity classification. For each detected deadlock, it produces a `DeadlockReport` with the lock cycle, the acquisition chains that form the cycle, and suggested fixes (e.g., "always acquire locks in the same order: A, B, C" or "use try_lock with timeout").

---

## 12. Proof Obligation Generation

### 12.1 Purpose and Intuition

Not every verification result is a clean Proven or Violated. Many invariants produce **Conditional** results — the IVE can verify safety under certain assumptions but cannot prove those assumptions hold universally. Rather than leaving these as ambiguous, the IVE generates **proof obligations**: precise, machine-checkable statements that, if proven, would promote the Conditional result to Proven. Each proof obligation encodes exactly what needs to be established, how difficult it is to prove, and what the programmer should do to resolve it.

Proof obligations bridge the gap between automatic verification and manual reasoning. The IVE can automatically discharge many obligations (e.g., single-threaded exclusivity is trivially proven). For others, it can suggest fixes that, if applied, would make the obligation provable. The programmer can also attach manual proofs or annotations to discharge obligations that the IVE cannot resolve automatically. The proof obligation system ensures that every Conditional result is tracked, categorized, and eventually resolved — no verification gap is left unattended.

### 12.2 Data Structures

**`ExclusivityProofObligation`** represents a single proof obligation arising from the IVE:

```
struct ExclusivityProofObligation:
    id: ObligationId
    invariant: InvariantKind          // Which invariant produced this obligation
    target: ObligationTarget          // The entity under scrutiny
    condition: String                 // What must be proven
    resolution_kind: ResolutionKind   // How the obligation can be resolved
    difficulty: ProofDifficulty        // How hard it is to prove
    suggested_fix: Option<SuggestedFix>  // What the programmer can do
    status: ObligationStatus          // Pending, Discharged, WontFix
    dependent_obligations: Vec<ObligationId>  // Obligations that depend on this one

enum InvariantKind:
    Liveness, Exclusivity, Interpretation, Origin, Cleanup

enum ObligationTarget:
    Access(AccessId)
    Derivation(DerivationId)
    Region(RegionId)
    ConflictPair(AccessId, AccessId)

enum ResolutionKind:
    AutomaticProof                  // IVE can prove this automatically
    RuntimeCheckInsertion           // Insert a runtime guard
    ProgrammerAnnotation            // Programmer must provide annotation
    StructuralFix                   // Code change needed
    CannotResolve                   // No known resolution

enum ProofDifficulty:
    Trivial     // Proven by construction (e.g., single-threaded access)
    Easy        // Simple proof by happens-before or basic algebra
    Medium      // Requires path analysis or constraint solving
    Hard        // Requires non-trivial mathematical reasoning
    Intractable // Beyond current automated proof capability

struct SuggestedFix:
    description: String              // Human-readable description
    fix_kind: FixKind                // Category of fix
    location: ProgramPoint           // Where to apply the fix
    code_snippet: Option<String>     // Suggested code change

enum FixKind:
    AddSynchronization              // Add lock, barrier, or channel
    AddRuntimeCheck                 // Insert bounds or null check
    AddAnnotation                   // Add lifetime, type, or safety annotation
    ReorderOperations               // Move code to establish ordering
    ChangeDataStructure             // Use a different data structure
    RefactorOwnership               // Restructure ownership to eliminate ambiguity
```

### 12.3 Obligation Generation Algorithm

```
function GENERATE_PROOF_OBLIGATIONS(verification_results: AllResults) -> Vec<ExclusivityProofObligation>:
    obligations = []

    for each (entity, result) in all_results:
        if result.status == Conditional:
            ob = ExclusivityProofObligation(
                id=FRESH_ID(),
                invariant=result.invariant,
                target=entity,
                condition=result.condition_description,
                resolution_kind=CLASSIFY_RESOLUTION(result),
                difficulty=ASSESS_DIFFICULTY(result),
                suggested_fix=GENERATE_SUGGESTED_FIX(result),
                status=Pending,
                dependent_obligations=[])
            obligations.append(ob)

        elif result.status == Violated:
            // Generate a structural fix obligation for violations
            ob = ExclusivityProofObligation(
                id=FRESH_ID(),
                invariant=result.invariant,
                target=entity,
                condition=format("Invariant {} must not be violated", result.invariant),
                resolution_kind=StructuralFix,
                difficulty=ASSESS_FIX_DIFFICULTY(result),
                suggested_fix=GENERATE_VIOLATION_FIX(result),
                status=Pending,
                dependent_obligations=[])
            obligations.append(ob)

    // Compute dependency graph between obligations
    obligations = COMPUTE_OBLIGATION_DEPENDENCIES(obligations)

    return obligations
```

### 12.4 Difficulty Assessment

The difficulty assessment heuristic considers multiple factors:

| Factor | Trivial | Easy | Medium | Hard | Intractable |
|--------|---------|------|--------|------|-------------|
| Invariant | Cleanup | Liveness | Interpretation | Exclusivity | — |
| Path complexity | Single path | Linear | Branching | Loop-dependent | Unbounded |
| Constraint type | None | Equality | Linear inequality | Non-linear | Quantified |
| Concurrency | None | Fork-join | Lock-based | Lock-free | Wait-free |
| Alias involvement | No | Must-alias | May-alias (1-2) | May-alias (many) | Unknown |

### 12.5 Complexity

- **Obligation generation:** O(|conditional_results| + |violated_results|) — linear in the number of non-proven results.
- **Difficulty assessment:** O(1) per obligation using the heuristic table above.
- **Suggested fix generation:** O(|suggested_fix_templates|) per obligation — typically O(1) with template matching.
- **Dependency computation:** O(|obligations|^2) in the worst case for pairwise dependency checking, but typically near-linear since most obligations are independent.
- **Overall:** O(|R_non_proven|), linear in the number of non-proven results, plus the dependency computation.

### 12.6 Output

A sorted list of `ExclusivityProofObligation` instances, ordered by difficulty (easiest first) and then by severity of the underlying violation. The programmer is presented with a prioritized list of things to fix or prove, with clear guidance on how to resolve each one.

---

## 13. Verification Pipeline Enhancements

### 13.1 Purpose and Intuition

The original verification pipeline (Section 6) runs all five invariant verifiers in a fixed dependency order. While correct, this design has two inefficiencies: (1) it does not exploit the opportunity for early termination — if the liveness verifier finds zero violations, the exclusivity verifier knows that all live accesses are safe and can skip certain checks; (2) it does not allow the pipeline to be reconfigured for different verification goals — sometimes the programmer only wants to check exclusivity, or wants a quick "smoke test" that runs the cheapest verifiers first. The enhanced pipeline introduces configurable execution order, early termination conditions, and per-invariant timing to address these issues.

The enhanced pipeline is driven by `AggregatorConfig`, which specifies the execution order, time budgets, and early-termination conditions. The `OPTIMAL_INVARIANT_ORDER` is a precomputed ordering that minimizes expected total verification time by running cheaper, more likely-to-fail invariants first, so that early termination can skip the expensive ones.

### 13.2 Data Structures

```
struct AggregatorConfig:
    invariant_order: Vec<InvariantKind>         // Execution order
    time_budgets: Map<InvariantKind, Duration>  // Per-invariant time limit
    early_termination: EarlyTerminationPolicy   // When to stop early
    parallel_independent: bool                   // Run independent invariants concurrently
    max_violations: Option<usize>               // Stop after N violations
    confidence_threshold: f64                    // Minimum acceptable confidence

enum EarlyTerminationPolicy:
    Never                          // Always run all invariants
    OnFirstViolation               // Stop as soon as any violation is found
    OnCriticalViolation            // Stop only on Critical severity violations
    OnConfidenceBelow(f64)         // Stop if overall confidence drops below threshold
    AfterMaxViolations(usize)      // Stop after N total violations

const OPTIMAL_INVARIANT_ORDER: [InvariantKind; 5] = [
    Cleanup,          // Cheapest: O(|R| × |paths|), most likely to find leaks
    Origin,           // Cheap: O(|D| × L), catches pointer provenance issues
    Liveness,         // Medium: O(|A| × k), catches use-after-free
    Interpretation,   // Medium: O(|A| × log |H|), catches type confusion
    Exclusivity,      // Most expensive: O(|A|^2 × |S|), data race detection
]
```

### 13.3 Enhanced Pipeline Algorithm

```
function ENHANCED_VERIFY_PROGRAM(scg: SCG, config: AggregatorConfig) -> VerificationReport:
    start_total = NOW()
    msg = BUILD_MSG(scg)
    bd_results = INFER_BEHAVIORAL_DESCRIPTORS(scg)
    msg.annotate_with_bd(bd_results)

    all_results = {}
    violations_found = 0
    all_proofs = {}
    all_counterexamples = {}
    invariant_timings = {}

    for each invariant in config.invariant_order:
        // Check time budget
        elapsed = NOW() - start_total
        if invariant in config.time_budgets:
            if elapsed > config.time_budgets[invariant]:
                all_results[invariant] = TIMED_OUT_RESULT(invariant)
                continue

        // Run the invariant verifier
        start_invariant = NOW()
        result = RUN_INVARIANT_VERIFIER(invariant, msg, all_results)
        invariant_timings[invariant] = NOW() - start_invariant

        all_results[invariant] = result

        // Count violations
        violations_found += COUNT_VIOLATIONS(result)

        // Early termination check
        if SHOULD_TERMINATE_EARLY(config, violations_found, all_results):
            break

        // Annotate MSG with results for downstream verifiers
        msg.annotate_with_results(invariant, result)

    // Generate proofs and counterexamples for completed verifiers
    for each (invariant, result) in all_results:
        if result != TIMED_OUT:
            all_proofs[invariant] = GENERATE_PROOFS(invariant, result)
            all_counterexamples[invariant] = GENERATE_COUNTEREXAMPLES(invariant, result)

    return VerificationReport(
        proofs=all_proofs,
        counterexamples=all_counterexamples,
        confidence=COMPUTE_CONFIDENCE_LEVELS(all_results),
        timings=invariant_timings,
        early_terminated=violations_found > 0 and
            config.early_termination != Never,
        summary=SUMMARIZE_RESULTS(all_results))
```

### 13.4 Early Termination Logic

```
function SHOULD_TERMINATE_EARLY(config, violations, results) -> bool:
    switch config.early_termination:
        case Never:
            return false
        case OnFirstViolation:
            return violations > 0
        case OnCriticalViolation:
            return any result has Critical severity violation
        case OnConfidenceBelow(threshold):
            return COMPUTE_OVERALL_CONFIDENCE(results) < threshold
        case AfterMaxViolations(max):
            return violations >= max
```

### 13.5 Per-Invariant Timing

The enhanced pipeline records wall-clock time for each invariant verification phase. This data serves two purposes: (1) it allows the programmer to identify which invariants are bottlenecking verification and optimize their code accordingly; (2) it feeds back into the `AggregatorConfig` to adjust time budgets and execution order for subsequent runs. Timing data is included in the `VerificationReport` and displayed in the IDE projection.

### 13.6 Complexity

- **With early termination on first violation:** O(cheapest_violation_cost) — may stop after the first invariant if it finds a violation.
- **With OPTIMAL_INVARIANT_ORDER and no early termination:** Same total complexity as the basic pipeline, but the expected time is lower because cheaper invariants run first.
- **Time budget enforcement:** O(1) overhead per invariant for the time check.
- **Overall:** The worst-case complexity is unchanged from Section 6, but the practical performance is significantly improved for programs with violations or when the programmer only needs a subset of invariants verified.

---

## 14. Cross-Invariant Dependencies

### 14.1 Purpose and Intuition

The basic pipeline treats invariant dependencies as a simple DAG: interpretation depends on BD, liveness depends on origin, exclusivity depends on liveness, and cleanup is independent. However, real verification reveals more subtle cross-invariant dependencies. An origin violation may invalidate a liveness proof that depends on the violated derivation. An interpretation violation may create an exclusivity violation if the wrong RepD causes the IVE to misclassify an access as a read when it is actually a write. The `InvariantDependencyGraph` captures these fine-grained dependencies and enables **impact analysis**: when one invariant's result changes, the IVE can determine precisely which other invariants need re-verification and which results can be preserved.

Without this dependency tracking, any change to one invariant's result would conservatively require re-verifying all downstream invariants. With fine-grained tracking, the IVE can limit re-verification to only those downstream results that actually depend on the changed upstream result. This is especially important for incremental verification (Section 7), where the cost of unnecessary re-verification directly impacts the user experience.

### 14.2 Data Structures

```
struct InvariantDependencyGraph:
    nodes: Map<DependencyNodeId, DependencyNode>
    edges: Map<(DependencyNodeId, DependencyNodeId), DependencyEdge>

struct DependencyNode:
    id: DependencyNodeId
    invariant: InvariantKind
    entity: EntityId                   // Access, Derivation, Region, etc.
    result: VerificationResult

struct DependencyEdge:
    source: DependencyNodeId
    target: DependencyNodeId
    dependency_kind: DependencyKind
    impact_strength: ImpactStrength    // How strongly the target depends on the source

enum DependencyKind:
    RegionResolution                   // Origin result determines which region an access targets
    LivenessClassification             // Liveness result determines if an access is live
    RepDClassification                 // Interpretation result determines the RepD of an access
    AccessMode                         // Whether the access is Read or Write
    BoundsInformation                  // Origin bounds affect liveness region scope
    SynchronizationScope               // Exclusivity depends on sync edge classification

enum ImpactStrength:
    Strong      // Source change always invalidates target
    Weak        // Source change may not invalidate target
    Conditional // Source change invalidates target only under certain conditions
```

### 14.3 Impact Analysis Algorithm

```
function ANALYZE_IMPACT(graph: InvariantDependencyGraph,
                        changed: Set<DependencyNodeId>) -> ImpactAnalysis:
    affected = set(changed)   // Start with directly changed nodes
    re_verification_plan = []

    // BFS from changed nodes through dependency edges
    queue = changed.to_queue()
    while queue is not empty:
        node = queue.dequeue()
        for each edge (node, successor) in graph.edges_from(node):
            if successor not in affected:
                if edge.impact_strength == Strong:
                    affected.add(successor)
                    queue.enqueue(successor)
                    re_verification_plan.append(ReVerificationStep(
                        target=successor,
                        reason=format("{} changed → {} must be re-verified",
                                     node, successor),
                        priority=HIGH))
                elif edge.impact_strength == Weak:
                    // May need re-verification; check if the change
                    // actually affects the successor's result
                    if CHANGE_AFFECTS_RESULT(node.result, successor.result, edge):
                        affected.add(successor)
                        queue.enqueue(successor)
                        re_verification_plan.append(ReVerificationStep(
                            target=successor,
                            reason=format("{} change may affect {}",
                                         node, successor),
                            priority=MEDIUM))
                elif edge.impact_strength == Conditional:
                    if CONDITION_HOLDS(edge.condition, graph):
                        affected.add(successor)
                        queue.enqueue(successor)
                        re_verification_plan.append(ReVerificationStep(
                            target=successor,
                            reason=format("{} change affects {} under condition",
                                         node, successor),
                            priority=LOW))

    return ImpactAnalysis(
        affected_nodes=affected,
        re_verification_plan=re_verification_plan.sorted_by(priority))
```

### 14.4 Re-Verification Planning

The impact analysis produces a prioritized re-verification plan. The plan specifies which invariant-entity pairs need re-verification and in what order. The IVE executes this plan during incremental verification, skipping any invariants and entities not in the plan. The plan respects the dependency ordering: if a Strong dependency indicates that a liveness result depends on an origin result, and the origin result has changed, the liveness re-verification is scheduled after the origin re-verification.

```
function EXECUTE_RE_VERIFICATION_PLAN(plan, msg, cached_results):
    new_results = copy(cached_results)

    for each step in plan:
        invariant = step.target.invariant
        entity = step.target.entity

        // Re-verify just this entity
        result = RUN_INVARIANT_VERIFIER(invariant, msg, new_results)
            .restricted_to([entity])

        // Update results and check for cascading impacts
        old_result = new_results[invariant][entity]
        new_results[invariant][entity] = result

        if old_result != result:
            // Result changed: check for additional downstream impacts
            cascaded = ANALYZE_IMPACT(graph, {step.target})
            for each cascade_step in cascaded.re_verification_plan:
                if cascade_step not in plan:
                    plan.append(cascade_step)  // Dynamically add steps

    return new_results
```

### 14.5 Complexity

- **Dependency graph construction:** O(|entities| × |dependencies_per_entity|), typically O(|entities|) since each entity has a bounded number of dependencies.
- **Impact analysis (BFS):** O(|affected_nodes| + |dependency_edges_from_affected|), linear in the size of the affected subgraph.
- **Re-verification plan execution:** O(|plan_steps| × |re_verification_cost_per_step|), where the per-step cost depends on the invariant being re-verified.
- **Cascading re-verification:** In the worst case, a single change can cascade to O(|entities|) re-verifications, but the impact strength classification limits this in practice. Strong dependencies are rare; most dependencies are Weak or Conditional.

---

## 15. Verification Debt

### 15.1 Purpose and Intuition

Not all verification results need to be resolved immediately. A programmer working on a new feature may introduce Conditional results or even violations that are temporarily acceptable — they represent **verification debt** that should be tracked and eventually resolved, much like technical debt in software engineering. The enhanced verification debt system extends the basic tracking of unresolved obligations with **scoring** (how serious is the debt?), **aging** (how long has it been unresolved?), and **auto-resolution** (can the IVE automatically resolve the debt when other changes make it possible?).

Verification debt is distinct from a simple list of unresolved obligations. It incorporates temporal information: a violation that has existed for 100 commits is more concerning than one introduced in the last commit. It incorporates severity weighting: a critical data race is more urgent than a medium-severity type confusion. And it incorporates resolution potential: an obligation that could be automatically resolved by an upcoming code change is less concerning than one that requires manual intervention. The debt system provides a dashboard that helps the programmer prioritize their verification work.

### 15.2 Data Structures

```
struct VerificationDebt:
    obligations: Map<ObligationId, DebtEntry>
    total_score: f64
    aging_policy: AgingPolicy

struct DebtEntry:
    obligation: ExclusivityProofObligation
    introduced_at: CommitId           // When the debt was created
    age_weight: f64                    // Increases over time
    severity_weight: f64               // Based on violation severity
    resolution_potential: f64          // 0.0–1.0 likelihood of auto-resolution
    auto_resolution_attempts: u32      // How many times auto-resolution was tried

enum AgingPolicy:
    Linear(rate: f64)                  // score += rate × age
    Exponential(rate: f64, cap: f64)   // score += min(rate^age, cap)
    StepFunction(thresholds: Vec<(u32, f64)>)  // score jumps at age thresholds

struct DebtScore:
    raw_score: f64                     // severity × age × resolution_potential
    normalized_score: f64              // 0.0–100.0 scale
    priority: DebtPriority

enum DebtPriority:
    Critical    // Must resolve immediately (e.g., exploitable data race)
    High        // Resolve within current sprint
    Medium      // Resolve within next few sprints
    Low         // Can defer indefinitely
    AutoResolvable  // IVE can resolve automatically
```

### 15.3 Debt Scoring Algorithm

```
function COMPUTE_DEBT_SCORE(entry: DebtEntry, policy: AgingPolicy) -> DebtScore:
    severity = SEVERITY_TO_WEIGHT(entry.obligation.difficulty)
    age = COMPUTE_AGE(entry.introduced_at)
    age_factor = APPLY_AGING_POLICY(age, policy)
    resolution_discount = 1.0 - entry.resolution_potential
        // Higher resolution potential → lower urgency

    raw_score = severity * age_factor * resolution_discount

    // Normalize to 0–100 scale
    normalized = min(raw_score * NORMALIZATION_CONSTANT, 100.0)

    // Classify priority
    priority = if normalized >= 80: Critical
               elif normalized >= 60: High
               elif normalized >= 30: Medium
               elif entry.resolution_potential >= 0.8: AutoResolvable
               else: Low

    return DebtScore(raw_score=raw_score, normalized_score=normalized, priority=priority)
```

### 15.4 Auto-Resolution

The IVE periodically attempts to automatically resolve verification debt. Auto-resolution succeeds when a code change (by the programmer or another tool) makes a previously unprovable obligation provable. The auto-resolution system runs as a background task:

```
function ATTEMPT_AUTO_RESOLUTION(debt: &mut VerificationDebt, msg: MSG):
    for each (id, entry) in debt.obligations:
        if entry.resolution_potential < 0.1:
            continue  // Very unlikely to auto-resolve; skip

        entry.auto_resolution_attempts += 1

        // Re-run the relevant invariant verifier
        result = RUN_INVARIANT_VERIFIER(entry.obligation.invariant, msg, {})

        // Check if the obligation is now provable
        entity_result = result[entry.obligation.target]
        if entity_result.status == Proven:
            // Debt resolved! Update the entry
            entry.obligation.status = Discharged
            entry.resolution_potential = 1.0
            DECREASE_TOTAL_SCORE(debt, entry)
        elif entity_result.status == Violated and
             entry.obligation.resolution_kind == StructuralFix:
            // A structural fix may now be simpler; update suggestion
            entry.obligation.suggested_fix = GENERATE_SUGGESTED_FIX(entity_result)
            entry.resolution_potential = min(entry.resolution_potential + 0.1, 0.9)
```

### 15.5 Complexity

- **Debt scoring:** O(|debt_entries|) — linear in the number of debt entries. Each entry requires O(1) computation.
- **Auto-resolution attempt:** O(|auto_resolvable_entries| × |re_verification_cost|), where the re-verification cost depends on the invariant and entity. This is bounded by the total verification cost, but typically much smaller because only a subset of entries are attempted.
- **Aging computation:** O(|debt_entries|) — applied as a batch update on each commit or verification cycle.
- **Overall:** The debt system adds O(|D|) overhead per verification cycle, where |D| is the number of debt entries. This is negligible compared to the verification cost itself.

### 15.6 Output

The verification debt system produces a **DebtDashboard** containing:
- Total debt score (normalized 0–100).
- Number of entries by priority level.
- Oldest unresolved obligations.
- Most recently auto-resolved obligations.
- Trend graph (debt score over time).
- Recommended resolution order based on priority and aging.

---

## 16. Error Recovery

### 16.1 Purpose and Intuition

The basic verification pipeline assumes that each invariant verifier produces a complete result for every entity it checks. In practice, verification may encounter errors: the SMT solver times out on a complex constraint, the path analysis exceeds its loop unrolling budget, or the MSG is inconsistent due to a bug in the compiler front-end. Without error recovery, a single error in one invariant verifier could crash the entire pipeline, losing all results from all other verifiers. The `ErrorCollector` and `PartialVerificationResult` system provides graceful degradation: when an error occurs, the IVE records it, preserves all results computed so far, and continues with the remaining verifiers and entities.

Error recovery is essential for the "always-verified" experience. The programmer should never be presented with a blank result or a cryptic error message. Instead, they should see which invariants were verified, which had errors, and what the errors mean. Partial results are still useful: knowing that 99% of accesses pass the liveness check and only one access encounters an error is far more informative than knowing nothing.

### 16.2 Data Structures

```
struct ErrorCollector:
    errors: Vec<VerificationError>
    warnings: Vec<VerificationWarning>
    error_limit: usize                 // Maximum errors before aborting the current verifier
    error_counts: Map<ErrorCategory, usize>

struct VerificationError:
    category: ErrorCategory
    invariant: Option<InvariantKind>
    entity: Option<EntityId>
    message: String
    recoverable: bool
    context: ErrorContext

struct VerificationWarning:
    invariant: InvariantKind
    entity: EntityId
    message: String
    confidence_degradation: f64         // How much confidence is reduced

enum ErrorCategory:
    SolverTimeout
    PathAnalysisBudgetExceeded
    InconsistentMSG
    MissingBDInformation
    CyclicDependency
    InternalError
    ExternalToolFailure

struct ErrorContext:
    program_point: Option<ProgramPoint>
    derivation_chain: Option<Vec<DerivationStep>>
    region: Option<RegionId>
    suggestion: Option<String>

struct PartialVerificationResult:
    completed_entities: Map<EntityId, VerificationResult>
    failed_entities: Map<EntityId, VerificationError>
    skipped_entities: Set<EntityId>          // Entities not checked due to errors
    coverage: f64                            // Fraction of entities successfully checked
    confidence: f64                          // Adjusted for errors and skips
```

### 16.3 Error-Resilient Verification Algorithm

```
function VERIFY_WITH_ERROR_RECOVERY(msg: MSG, config: AggregatorConfig) ->
        (PartialVerificationResult, ErrorCollector):
    collector = ErrorCollector(error_limit=config.max_errors_per_invariant)
    partial = PartialVerificationResult(
        completed={}, failed={}, skipped={}, coverage=0.0, confidence=1.0)

    for each invariant in config.invariant_order:
        entities = GET_ENTITIES_FOR_INVARIANT(invariant, msg)

        for each entity in entities:
            // Check error limit
            if collector.error_count(invariant) >= collector.error_limit:
                // Too many errors: skip remaining entities for this invariant
                for remaining in entities.not_yet_checked():
                    partial.skipped.insert(remaining)
                break

            // Attempt verification with error handling
            match VERIFY_ENTITY(invariant, entity, msg):
                case Ok(result):
                    partial.completed[entity] = result
                case Err(error):
                    collector.errors.append(error)
                    if error.recoverable:
                        partial.failed[entity] = error
                    else:
                        // Non-recoverable: skip all remaining entities
                        for remaining in entities.not_yet_checked():
                            partial.skipped.insert(remaining)
                        break

        // Compute coverage for this invariant
        total = len(entities)
        checked = len(partial.completed) + len(partial.failed)
        coverage_per_invariant = checked as f64 / total as f64

    // Compute overall coverage and confidence
    partial.coverage = COMPUTE_OVERALL_COVERAGE(partial)
    partial.confidence = COMPUTE_ADJUSTED_CONFIDENCE(partial, collector)

    return (partial, collector)
```

### 16.4 Confidence Degradation

Errors reduce the overall confidence of the verification result. The degradation formula accounts for both the fraction of entities that could not be verified and the severity of the errors:

```
function COMPUTE_ADJUSTED_CONFIDENCE(partial, collector) -> f64:
    base_confidence = partial.coverage  // Starts with coverage ratio

    // Reduce confidence based on error severity
    for each error in collector.errors:
        switch error.category:
            case SolverTimeout:
                base_confidence *= 0.9   // Timeout: result may exist but wasn't found
            case InconsistentMSG:
                base_confidence *= 0.5   // MSG inconsistency: results unreliable
            case PathAnalysisBudgetExceeded:
                base_confidence *= 0.8   // Approximation: sound but incomplete
            case InternalError:
                base_confidence *= 0.3   // Internal error: results may be wrong
            case _:
                base_confidence *= 0.7   // Unknown error

    return max(base_confidence, 0.0)
```

### 16.5 Complexity

- **Error collection overhead:** O(1) per error, O(|errors|) total — negligible.
- **Error limit checking:** O(1) per entity — a simple counter comparison.
- **Coverage computation:** O(|invariants|) — one division per invariant.
- **Confidence degradation:** O(|errors|) — one multiplication per error.
- **Overall:** The error recovery system adds O(|errors|) overhead to the total verification cost, which is negligible in all practical scenarios. The main cost is the re-verification of failed entities when the error condition is resolved (e.g., when the SMT solver is given more time).

### 16.6 Output

The error recovery system produces a `PartialVerificationResult` that includes all successfully verified entities, all errors encountered, all skipped entities, and the adjusted confidence level. The programmer sees: (1) which invariants passed, (2) which had errors with detailed error messages and suggestions, (3) what fraction of the program was successfully verified, and (4) the overall confidence in the verification result.

---

## 17. Incremental Verification Enhancements

### 17.1 Purpose and Intuition

The basic incremental verification system (Section 7) detects changes to the SCG and re-verifies only the affected invariants. While effective, it has three limitations: (1) the change detection is coarse-grained — a single node change may be classified as affecting multiple invariants even when only one is truly impacted; (2) the verification cache has no eviction policy and may grow unbounded; (3) there is no mechanism for detecting when an incremental re-verification result is inconsistent with the cached results, which can happen if the change affects cross-entity dependencies that the cache doesn't track. The enhanced incremental verification system introduces fine-grained `ChangeDetector`, bounded `VerificationCache`, and `IncrementalVerifier` with consistency checks.

The enhanced system is designed for the "always-verified" editing experience where the programmer makes frequent small changes. The target is sub-1-second re-verification for single-function edits, as measured by the `IncrementalMetrics` system. This target is achievable because most edits affect only a small number of regions and derivations, and the incremental verifier can re-use the vast majority of cached results.

### 17.2 Data Structures

```
struct ChangeDetector:
    old_scg: SCG
    new_scg: SCG

struct ChangeSet:
    added_nodes: Set<NodeId>
    removed_nodes: Set<NodeId>
    modified_nodes: Set<(NodeId, NodeDelta)>
    added_edges: Set<(NodeId, NodeId)>
    removed_edges: Set<(NodeId, NodeId)>
    affected_regions: Set<RegionId>
    affected_derivations: Set<DerivationId>

struct NodeDelta:
    field: String
    old_value: String
    new_value: String

struct VerificationCache:
    entries: HashMap<CacheKey, CacheEntry>
    max_size: usize                       // Bounded cache size
    hits: u64
    misses: u64

struct CacheKey:
    region_id: RegionId
    invariant: InvariantKind
    msg_hash: u64                         // Hash of the MSG subgraph

struct CacheEntry:
    result: VerificationResult
    timestamp: Instant
    access_count: u64
    msg_fingerprint: u64                  // For consistency validation

struct IncrementalMetrics:
    change_detection_time: Duration
    delta_computation_time: Duration
    re_verification_time: Duration
    total_time: Duration
    meets_target: bool                    // total_time < 1 second

struct IncrementalVerificationResult:
    result: PartialVerificationResult
    re_verified_invariants: Set<InvariantKind>
    skipped_invariants: Set<InvariantKind>
    nodes_re_checked: usize
    total_nodes: usize
    savings_ratio: f64                    // 1.0 - (nodes_re_checked / total_nodes)
```

### 17.3 Enhanced Change Detection Algorithm

```
function DETECT_CHANGES(detector: ChangeDetector) -> ChangeSet:
    changes = ChangeSet()

    // Phase 1: Node-level diffing
    old_nodes = detector.old_scg.node_ids()
    new_nodes = detector.new_scg.node_ids()

    changes.added_nodes = new_nodes - old_nodes
    changes.removed_nodes = old_nodes - new_nodes

    // For common nodes, check for property modifications
    for each node_id in old_nodes ∩ new_nodes:
        old_node = detector.old_scg.get_node(node_id)
        new_node = detector.new_scg.get_node(node_id)
        if old_node != new_node:
            delta = COMPUTE_NODE_DELTA(old_node, new_node)
            changes.modified_nodes.insert((node_id, delta))

            // Determine affected regions and derivations
            if delta.affects_region_bounds():
                changes.affected_regions.insert(old_node.region_id)
            if delta.affects_derivation():
                changes.affected_derivations.insert(old_node.derivation_id)

    // Phase 2: Edge-level diffing
    old_edges = detector.old_scg.edge_set()
    new_edges = detector.new_scg.edge_set()

    changes.added_edges = new_edges - old_edges
    changes.removed_edges = old_edges - new_edges

    // Phase 3: Compute affected invariants
    return changes


function COMPUTE_AFFECTED_INVARIANTS(changes: ChangeSet) -> Set<InvariantKind>:
    affected = set()

    if not changes.affected_regions.is_empty():
        affected.insert(Liveness)
        affected.insert(Cleanup)

    if not changes.affected_derivations.is_empty():
        affected.insert(Origin)
        affected.insert(Liveness)  // Origin changes cascade to liveness

    if not changes.added_edges.is_empty() or not changes.removed_edges.is_empty():
        affected.insert(Exclusivity)

    if not changes.removed_nodes.is_empty():
        affected.insert(Cleanup)

    return affected
```

### 17.4 Bounded Verification Cache

The enhanced cache uses an LRU (Least Recently Used) eviction policy to bound memory usage:

```
function CACHE_LOOKUP(cache: &mut VerificationCache, key: CacheKey) -> Option<VerificationResult>:
    match cache.entries.get_mut(&key):
        Some(entry) =>
            // Validate consistency: check that the MSG subgraph hash matches
            if entry.msg_fingerprint == key.msg_hash:
                entry.access_count += 1
                cache.hits += 1
                return Some(entry.result.clone())
            else:
                // Inconsistent: evict and return miss
                cache.entries.remove(&key)
                cache.misses += 1
                return None
        None =>
            cache.misses += 1
            return None


function CACHE_UPDATE(cache: &mut VerificationCache, key: CacheKey, result: VerificationResult):
    if cache.entries.len() >= cache.max_size:
        // Evict LRU entry
        lru_key = FIND_LRU_ENTRY(cache)
        cache.entries.remove(&lru_key)

    cache.entries.insert(key, CacheEntry(
        result=result,
        timestamp=NOW(),
        access_count=1,
        msg_fingerprint=key.msg_hash))
```

### 17.5 Incremental Verifier Integration

```
function INCREMENTAL_VERIFY(verifier: IncrementalVerifier, new_scg: SCG) ->
        IncrementalVerificationResult:
    start = NOW()

    // Step 1: Detect changes
    detector = ChangeDetector(old=verifier.last_scg, new=new_scg)
    changes = DETECT_CHANGES(detector)
    detection_time = NOW() - start

    // Step 2: If no changes, return cached results
    if changes.is_empty():
        return IncrementalVerificationResult(
            result=verifier.cached_results,
            re_verified_invariants={},
            skipped_invariants=ALL_INVARIANTS,
            nodes_re_checked=0,
            total_nodes=verifier.last_scg.node_count(),
            savings_ratio=1.0)

    // Step 3: Compute affected invariants
    affected_invariants = COMPUTE_AFFECTED_INVARIANTS(changes)

    // Step 4: Invalidate cache entries for affected regions
    for each region in changes.affected_regions:
        verifier.cache.invalidate(region)

    // Step 5: Rebuild affected MSG subgraph
    delta_start = NOW()
    msg = INCREMENTAL_MSG_REBUILD(changes, verifier.last_msg)
    delta_time = NOW() - delta_start

    // Step 6: Re-verify only affected invariants
    verify_start = NOW()
    new_results = copy(verifier.cached_results)
    re_checked = 0

    for each invariant in affected_invariants:
        entities = GET_AFFECTED_ENTITIES(invariant, changes, msg)
        for each entity in entities:
            // Check cache first
            key = CacheKey(entity.region, invariant, HASH_SUBGRAPH(entity, msg))
            match CACHE_LOOKUP(verifier.cache, key):
                Some(cached) =>
                    new_results[invariant][entity] = cached
                None =>
                    result = VERIFY_ENTITY(invariant, entity, msg, new_results)
                    new_results[invariant][entity] = result
                    CACHE_UPDATE(verifier.cache, key, result)
                    re_checked += 1

    verify_time = NOW() - verify_start
    total_time = NOW() - start

    return IncrementalVerificationResult(
        result=new_results,
        re_verified_invariants=affected_invariants,
        skipped_invariants=ALL_INVARIANTS - affected_invariants,
        nodes_re_checked=re_checked,
        total_nodes=new_scg.node_count(),
        savings_ratio=1.0 - (re_checked as f64 / new_scg.node_count() as f64))
```

### 17.6 Complexity

- **Change detection:** O(|delta_nodes| + |delta_edges|) — proportional to the change size, not the program size.
- **Affected invariant computation:** O(|changes|) — constant per change.
- **Cache lookup:** O(1) amortized with hash-based lookup. Consistency validation is O(1) (hash comparison).
- **Cache update:** O(1) amortized; O(log |cache|) for LRU eviction when the cache is full.
- **Re-verification:** O(|affected_entities| × |verification_cost_per_entity|), which is O(δ × cost) where δ is the change size.
- **Overall:** O(δ × log N) where δ is the change size and N is the program size. The log factor comes from cache indexing and dependency lookups. This achieves the sub-1-second target for single-function edits.

---

## Appendix A: Data Structure Summary

| Structure | Fields | Purpose |
|-----------|--------|---------|
| `Region` | `id`, `alloc_point`, `free_point`, `address_range`, `size`, `leak_status`, `repd_history` | Represents a contiguous allocated memory range |
| `Access` | `target`, `size`, `kind` (Read/Write/Execute/Free), `program_point`, `thread`, `expected_RepD` | Represents a single memory access operation |
| `Derivation` | `source`, `operation` (Offset/Element/Field/Arithmetic/Cast), `parameters`, `result` | Represents how one address is derived from another |
| `SyncEdge` | `src_point`, `dst_point`, `kind` (LockAcquire/LockRelease/ChannelSend/ChannelRecv/Fork/Join/Barrier) | Represents inter-thread synchronization |
| `RepD` | `size`, `alignment`, `fields`, `interpretation` (Uninitialized/Bytes/Typed/Struct) | Represents the memory layout and valid interpretation |
| `LivenessResult` | `status` (Proven/Violated/Conditional), `reason`, `counterexample`, `confidence` | Result of liveness verification for one access |
| `ExclusivityResult` | `status`, `reason`, `counterexample`, `confidence` | Result of exclusivity verification for one conflict pair |
| `InterpretationResult` | `status`, `reason`, `counterexample`, `confidence` | Result of interpretation verification for one access |
| `OriginResult` | `status`, `reason`, `counterexample`, `confidence` (tiered) | Result of origin verification for one derivation |
| `CleanupResult` | `status`, `reason`, `counterexample`, `confidence` | Result of cleanup verification for one region |
| `DerivationAliasInfo` | `derivation_a`, `derivation_b`, `alias_kind`, `overlap_range`, `confidence`, `program_points` | Aliasing relationship between two derivations (§8) |
| `AccessIntervalTree` | `root`, `size` | Augmented AVL tree for O(log n) interval overlap queries (§9) |
| `IntervalNode` | `interval`, `max_hi`, `left`, `right`, `height`, `data` | Node in the interval tree (§9) |
| `DeepConfusionKind` | UnionDiscriminatorMismatch / EnumVariantMismatch / DiscriminatorCorrupted / CrossVariantFieldAccess | Categories of deep type confusion (§10) |
| `UnionDiscriminatorState` | `location`, `current_discriminator`, `discriminator_type`, `variant_map`, `write_history` | Tracks union discriminator at a memory location (§10) |
| `EnumVariantState` | `location`, `active_variant`, `variant_repds`, `transition_history` | Tracks enum variant at a memory location (§10) |
| `HappensBeforeGraph` | `edges`, `closure`, `lock_graph` | Extended HB relation with fine-grained edge types (§11) |
| `LockAcquisitionGraph` | `nodes`, `edges` | Lock ordering for deadlock detection (§11) |
| `AcquisitionRecord` | `thread`, `held_lock`, `acquired_lock`, `program_point` | Single lock acquisition event (§11) |
| `ExclusivityProofObligation` | `id`, `invariant`, `target`, `condition`, `resolution_kind`, `difficulty`, `suggested_fix`, `status`, `dependent_obligations` | Proof obligation for conditional/violated results (§12) |
| `SuggestedFix` | `description`, `fix_kind`, `location`, `code_snippet` | Machine-generated fix suggestion (§12) |
| `AggregatorConfig` | `invariant_order`, `time_budgets`, `early_termination`, `parallel_independent`, `max_violations`, `confidence_threshold` | Pipeline configuration (§13) |
| `InvariantDependencyGraph` | `nodes`, `edges` | Fine-grained cross-invariant dependency tracking (§14) |
| `DependencyEdge` | `source`, `target`, `dependency_kind`, `impact_strength` | Edge in the dependency graph (§14) |
| `DebtEntry` | `obligation`, `introduced_at`, `age_weight`, `severity_weight`, `resolution_potential`, `auto_resolution_attempts` | Verification debt tracking entry (§15) |
| `ErrorCollector` | `errors`, `warnings`, `error_limit`, `error_counts` | Error collection for graceful degradation (§16) |
| `PartialVerificationResult` | `completed_entities`, `failed_entities`, `skipped_entities`, `coverage`, `confidence` | Partial result with coverage info (§16) |
| `ChangeDetector` | `old_scg`, `new_scg` | Detects fine-grained changes between SCG versions (§17) |
| `ChangeSet` | `added_nodes`, `removed_nodes`, `modified_nodes`, `added_edges`, `removed_edges`, `affected_regions`, `affected_derivations` | Granular change record (§17) |
| `VerificationCache` | `entries`, `max_size`, `hits`, `misses` | Bounded LRU cache with consistency validation (§17) |
| `IncrementalMetrics` | `change_detection_time`, `delta_computation_time`, `re_verification_time`, `total_time`, `meets_target` | Performance metrics for incremental verification (§17) |
| `IncrementalVerifier` | Encapsulates `ChangeDetector`, `VerificationCache`, and re-verification logic | Top-level incremental verification driver (§17) |

## Appendix B: Invariant Dependency Matrix

| Invariant | Depends on BD | Depends on Origin | Depends on Liveness | Independent |
|-----------|:---:|:---:|:---:|:---:|
| Interpretation | ✓ | | | |
| Origin | | | | ✓ |
| Liveness | | ✓ | | |
| Exclusivity | | | ✓ | |
| Cleanup | | | | ✓ |

## Appendix C: Complexity Summary

### Core Invariants (Sections 1–5)

| Invariant | Worst Case | Practical (with optimizations) | Incremental |
|-----------|-----------|-------------------------------|-------------|
| Liveness | O(\|A\| × \|R\| × \|P\|) | O(\|A\| + \|R\| × k) | O(δ × log N) |
| Exclusivity | O(\|A\|^2 × \|S\|) | O(Σ_rc \|A_rc\|^2 × \|S_rc\|) | O(δ × log N) |
| Interpretation | O(\|A\| × \|H\|) | O(\|A\| × log \|H\|) | O(δ × log N) |
| Origin | O(\|D\| × L) | O(\|D\| × L_avg) | O(δ × log N) |
| Cleanup | O(\|R\| × \|P\|) | O(\|R\| × \|cfg\|) | O(δ × log N) |

### Wave 1 IVE Capabilities (Sections 8–17)

| Capability | Worst Case | Practical | Incremental |
|-----------|-----------|-----------|-------------|
| Multi-Pointer Aliasing (§8) | O(\|D\|^2) | O(\|D\| × α(\|D\|)) per region | O(δ × α(δ)) |
| Interval Tree (§9) | O(n log n) build | O(log n + k) query | O(log n + k) |
| Deep Type Confusion (§10) | O(\|A_tagged\| + \|W_tagged\|) | O(\|A_tagged\|) | O(δ) |
| Concurrent Exclusivity (§11) | O(\|A\|^2 / \|R\|) + O(\|L\| + \|E_L\|) | Near-linear with interval tree | O(δ × log N) |
| Proof Obligations (§12) | O(\|R_non_proven\|^2) | O(\|R_non_proven\|) | O(δ) |
| Pipeline Enhancements (§13) | Same as core pipeline | Cheapest-first with early termination | O(cheapest violation) |
| Cross-Invariant Deps (§14) | O(\|entities\|^2) | O(\|affected\|) | O(δ × deps) |
| Verification Debt (§15) | O(\|D\|) per cycle | O(\|auto_resolvable\|) | O(\|D\|) |
| Error Recovery (§16) | O(\|errors\|) overhead | Negligible | O(\|errors\|) |
| Incremental Verification (§17) | O(δ × log N) | Sub-1s for single-function edit | O(δ × log N) |

Where: A = accesses, R = regions, P = paths, S = sync edges, H = RepD history size, D = derivations, L = max chain length, N = program size, δ = change size, k = overlap results per query, α = inverse Ackermann function.

---

*End of VUMA Verification Algorithm Specification*
