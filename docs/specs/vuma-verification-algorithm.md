# VUMA Verification Algorithm Specification

**Document ID:** VUMA-SPEC-VA-001
**Author:** Agent W1-27
**Date:** 2026-03-04
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

## Appendix B: Invariant Dependency Matrix

| Invariant | Depends on BD | Depends on Origin | Depends on Liveness | Independent |
|-----------|:---:|:---:|:---:|:---:|
| Interpretation | ✓ | | | |
| Origin | | | | ✓ |
| Liveness | | ✓ | | |
| Exclusivity | | | ✓ | |
| Cleanup | | | | ✓ |

## Appendix C: Complexity Summary

| Invariant | Worst Case | Practical (with optimizations) | Incremental |
|-----------|-----------|-------------------------------|-------------|
| Liveness | O(\|A\| × \|R\| × \|P\|) | O(\|A\| + \|R\| × k) | O(δ × log N) |
| Exclusivity | O(\|A\|^2 × \|S\|) | O(Σ_rc \|A_rc\|^2 × \|S_rc\|) | O(δ × log N) |
| Interpretation | O(\|A\| × \|H\|) | O(\|A\| × log \|H\|) | O(δ × log N) |
| Origin | O(\|D\| × L) | O(\|D\| × L_avg) | O(δ × log N) |
| Cleanup | O(\|R\| × \|P\|) | O(\|R\| × \|cfg\|) | O(δ × log N) |

Where: A = accesses, R = regions, P = paths, S = sync edges, H = RepD history size, D = derivations, L = max chain length, N = program size, δ = change size.

---

*End of VUMA Verification Algorithm Specification*
