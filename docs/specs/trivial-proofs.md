# VUMA Trivial Program Invariant Proofs

**Document:** VUMA-SPEC-TRIVIAL-001
**Author:** Parham Khairkhah

**References:** Proposal §2.9 (Safety Through Restriction Fallacy), §3.6 (Verified-Unsafe Memory Access)

---

> **Implementation note (2026-07):** This spec lists Access operations as 3 (Read/Write/Free) and uses an `Unborn` region status not listed in its own status enumeration. Cross-spec inconsistency: `vuma-invariants-spec.md` lists 2 Access kinds (Read/Write), `vuma-verification-algorithm.md` lists 4 (Read/Write/Execute/Free). Consult the source `AccessKind` enum for the definitive list. The proof infrastructure is implemented in `src/proof/src/` (15 files: liveness_proofs, exclusivity_proofs, interpretation_proofs, origin_proofs, cleanup_proofs, plus checker, rules, tactics, judgment, models, counterexample, composition, serialization).

## Notation and Conventions

We adopt the following formal notation for the Memory State Graph (MSG) and invariant proofs.

### MSG Components

An MSG is a tuple `M = (R, D, A, S)` where:

- **R** is the set of *regions*. Each region `ρ ∈ R` is a tuple `(id, base, size, status)` where `status ∈ {Allocated, Freed, Stack, Mapped}`. We write `ρ.status` for the status and `ρ.size` for the byte count.
- **D** is the set of *derivation edges*. Each `d ∈ D` is a tuple `(addr_src, addr_dst, offset, region)` recording that `addr_dst = addr_src + offset` and both pointers derive from `region`. We write `root(d)` for the region at the head of the derivation chain.
- **A** is the set of *access records*. Each `a ∈ A` is a tuple `(addr, op, width, program_point)` where `op ∈ {Read, Write, Free}` and `width` is the number of bytes touched. We write `a.pp` for the program point.
- **S** is the *region state map*: a function from program points to region status, `S : PP → (R → {Allocated, Freed, Unborn})`.

### Program Points

Program points are labeled `L0, L1, L2, ...` corresponding to sequential statement execution. At each program point the region state map records the status of every region.

### Invariant Definitions

We restate the five VUMA global invariants from §3.6.2 in formal terms:

1. **Liveness:** For every access `a ∈ A`, the region `ρ` containing `a.addr` satisfies `S(a.pp)(ρ) = Allocated`.
2. **Exclusivity:** For every pair of accesses `a₁, a₂ ∈ A` occurring at the same program point (concurrently), if `a₁.op = Write` or `a₂.op = Write`, then the byte ranges `[a₁.addr, a₁.addr + a₁.width)` and `[a₂.addr, a₂.addr + a₂.width)` do not overlap.
3. **Interpretation:** For every access `a ∈ A`, the representation descriptor (RepD) at the target bytes is consistent with the operation. That is, the access width `a.width` does not exceed the region size minus the offset, and the byte range `[a.addr, a.addr + a.width)` lies entirely within a single allocated region with a valid RepD.
4. **Origin:** For every address used in an access `a ∈ A`, there exists a derivation chain `d₁ → d₂ → ... → dₖ` in D such that `root(d₁)` is a region in R (i.e., the address traces back to a valid allocation).
5. **Cleanup:** For every region `ρ ∈ R`, either `∃ a ∈ A : a.op = Free ∧ a.addr = ρ.base`, or `ρ` is explicitly marked as intentionally leaked.

---

## Program 1: Simple Allocation

```
L0: r = allocate(8)
L1: *r = 42
L2: v = *r
L3: free(r)
```

### 1.1 MSG Construction

**Regions:**

| ID  | Base | Size | Status     |
|-----|------|------|------------|
| ρ₁  | r    | 8    | Allocated  |

**Derivation edges:**

| Source   | Destination | Offset | Region |
|----------|-------------|--------|--------|
| (alloc)  | r           | 0      | ρ₁     |

There are no derived pointers beyond `r` itself. The address `r` is a root pointer directly from the allocation site.

**Access records:**

| Addr | Op    | Width | Program Point |
|------|-------|-------|---------------|
| r    | Write | 8     | L1            |
| r    | Read  | 8     | L2            |
| r    | Free  | —     | L3            |

**Region state map S:**

| Program Point | ρ₁.status  |
|---------------|------------|
| L0 (after)    | Allocated  |
| L1 (after)    | Allocated  |
| L2 (after)    | Allocated  |
| L3 (after)    | Freed      |

### 1.2 Liveness Proof

**Claim:** For every access `a ∈ A`, `S(a.pp)(ρ₁) = Allocated`.

**Proof:** We enumerate the accesses:

- Access at L1: `*r = 42` is a Write to `r`. At program point L1, `S(L1)(ρ₁) = Allocated` because the allocation at L0 set `ρ₁.status = Allocated` and no free has occurred between L0 and L1. Therefore the liveness condition is satisfied.

- Access at L2: `v = *r` is a Read from `r`. At program point L2, `S(L2)(ρ₁) = Allocated` because the only mutation to ρ₁'s status was the allocation at L0, and no free has occurred prior to L2. Therefore the liveness condition is satisfied.

- Access at L3: `free(r)` is a Free of `r`. At program point L3, `S(L3)(ρ₁) = Allocated` because the status is still Allocated immediately before the free operation takes effect. The free operation itself transitions the status to Freed, but the invariant requires the region to be Allocated *at* the point of access, which it is. Therefore the liveness condition is satisfied.

There are no further accesses after L3. No access targets a Freed region. **Liveness holds.** □

### 1.3 Exclusivity Proof

**Claim:** No two concurrent accesses conflict.

**Proof:** This program is single-threaded and sequential. Every access occurs at a distinct program point (L1, L2, L3). There are no concurrent accesses. The exclusivity invariant requires that for any pair of accesses `a₁, a₂` occurring at the same program point, if either is a Write, their byte ranges must not overlap. Since there are no pairs of concurrent accesses, the antecedent of the condition is vacuously false. By vacuous truth, **exclusivity holds.** □

### 1.4 Interpretation Proof

**Claim:** Every access respects a valid representation descriptor.

**Proof:** The operation `*r = 42` at L1 writes a u64 value (8 bytes) into the region starting at address `r`. The region ρ₁ has size 8 bytes. The write width is 8 bytes, and the offset within the region is 0. We verify: `0 + 8 ≤ 8` (the write does not exceed the region boundary). The representation descriptor at `r` is `u64`, which occupies exactly 8 bytes. The write is consistent with this RepD.

The operation `v = *r` at L2 reads a u64 value (8 bytes) from address `r`. The region ρ₁ has size 8 bytes. The read width is 8 bytes, offset 0. We verify: `0 + 8 ≤ 8`. The RepD at `r` is `u64` (established by the write at L1), and the read is consistent. The memory at this point is initialized (written at L1), so there is no uninitialized-read violation.

The `free(r)` at L3 is a deallocation, not a data access; it does not interpret bytes. **Interpretation holds.** □

### 1.5 Origin Proof

**Claim:** Every address used in an access traces back to a valid allocation.

**Proof:** The sole address used in this program is `r`. The derivation edge in D records that `r` was produced by the allocation `allocate(8)` at L0, creating region ρ₁. The derivation chain has length 1: `r → ρ₁ (root)`. Since ρ₁ ∈ R is a valid allocated region, the origin of `r` is established. There are no other addresses in the access set. **Origin holds.** □

### 1.6 Cleanup Proof

**Claim:** Every allocated region is eventually freed.

**Proof:** Region ρ₁ is allocated at L0. The access record at L3 records `free(r)` where `r = ρ₁.base`. Therefore ρ₁ is freed. There are no other regions. **Cleanup holds.** □

### 1.7 Verdict

All five VUMA invariants hold for Program 1. The program is **SAFE**.

---

## Program 2: Use-After-Free

```
L0: r = allocate(8)
L1: *r = 42
L2: free(r)
L3: v = *r       // VIOLATION
```

### 2.1 MSG Construction

**Regions:**

| ID  | Base | Size | Status     |
|-----|------|------|------------|
| ρ₁  | r    | 8    | Allocated  |

**Derivation edges:**

| Source   | Destination | Offset | Region |
|----------|-------------|--------|--------|
| (alloc)  | r           | 0      | ρ₁     |

**Access records:**

| Addr | Op    | Width | Program Point |
|------|-------|-------|---------------|
| r    | Write | 8     | L1            |
| r    | Free  | —     | L2            |
| r    | Read  | 8     | L3            |

**Region state map S:**

| Program Point | ρ₁.status  |
|---------------|------------|
| L0 (after)    | Allocated  |
| L1 (after)    | Allocated  |
| L2 (after)    | Freed      |
| L3 (after)    | Freed      |

### 2.2 Liveness — VIOLATION

**Claim:** The liveness invariant is violated.

**Proof:** Consider the access at L3: `v = *r` is a Read from address `r`. The address `r` lies within region ρ₁ (by the derivation edge). At program point L3, the region state map gives `S(L3)(ρ₁) = Freed`, because the `free(r)` at L2 transitioned ρ₁'s status from Allocated to Freed. The liveness invariant requires `S(a.pp)(ρ) = Allocated` for every access. Since `S(L3)(ρ₁) = Freed ≠ Allocated`, the invariant is violated.

**Counterexample execution path:** `L0 → L1 → L2 → L3`. After L2, region ρ₁ is Freed. The read at L3 targets this freed region. This is a classic use-after-free bug. □

### 2.3 Exclusivity

**Claim:** Exclusivity holds (trivially).

**Proof:** The program is single-threaded and sequential. No concurrent accesses exist. The exclusivity condition is vacuously satisfied. However, this does not redeem the program — exclusivity is orthogonal to liveness. □

### 2.4 Interpretation

**Claim:** Interpretation is vacuously unsatisfied at L3 because liveness fails first.

**Note:** The interpretation invariant is technically also violated at L3, because reading from freed memory has no valid representation descriptor. However, the liveness violation is the primary and more fundamental error. The interpretation check would fail because the RepD at `r` is undefined after free — the region's metadata has been invalidated. In a layered verification, liveness is checked first; interpretation need not be checked for dead regions.

### 2.5 Origin

**Claim:** Origin holds.

**Proof:** The address `r` traces back to `allocate(8)` at L0 via the derivation edge. The origin chain is valid regardless of the region's current status — origin is about *provenance*, not *liveness*. □

### 2.6 Cleanup

**Claim:** Cleanup holds.

**Proof:** Region ρ₁ is freed at L2. The cleanup invariant is satisfied. □

### 2.7 Verdict

**LIVENESS VIOLATED** at L3. The program is **UNSAFE**.

**Counterexample path:** `L0 (alloc) → L1 (write) → L2 (free) → L3 (read freed region ρ₁)`

---

## Program 3: Out-of-Bounds

```
L0: r = allocate(8)
L1: *(r + 100) = 42   // VIOLATION
```

### 3.1 MSG Construction

**Regions:**

| ID  | Base | Size | Status     |
|-----|------|------|------------|
| ρ₁  | r    | 8    | Allocated  |

**Derivation edges:**

| Source   | Destination | Offset | Region |
|----------|-------------|--------|--------|
| (alloc)  | r           | 0      | ρ₁     |
| r        | r + 100     | 100    | ρ₁     |

The second derivation edge records that `r + 100` was derived from `r` with offset 100, within region ρ₁.

**Access records:**

| Addr     | Op    | Width | Program Point |
|----------|-------|-------|---------------|
| r + 100  | Write | 8     | L1            |

**Region state map S:**

| Program Point | ρ₁.status  |
|---------------|------------|
| L0 (after)    | Allocated  |
| L1 (after)    | Allocated  |

### 3.2 Origin — VIOLATION

**Claim:** The origin invariant is violated.

**Proof:** The address `r + 100` is used in the write at L1. The derivation chain traces `r + 100` back to `r` with offset 100, and `r` back to the allocation at L0 creating region ρ₁ of size 8 bytes. The origin invariant requires not merely that the address traces back to an allocation, but that the *derived pointer falls within the bounds of its root region*. Specifically, for a derived pointer `base + offset` rooted at region ρ with size `s`, we require `0 ≤ offset` and `offset + access_width ≤ s`.

Here: `offset = 100`, `access_width = 8`, `s = 8`. We check: `100 + 8 ≤ 8`? No — `108 ≤ 8` is false. The derived address `r + 100` exceeds the bounds of the region ρ₁ from which it was derived. The origin invariant, properly construed, requires that every derived pointer reference a location within the region of its allocation root. This bound is violated.

**Counterexample execution path:** `L0 → L1`. The derived pointer `r + 100` targets address `ρ₁.base + 100`, but ρ₁ spans only `[ρ₁.base, ρ₁.base + 8)`. The address `ρ₁.base + 100` lies outside this range. □

### 3.3 Liveness

**Claim:** Liveness is also violated.

**Proof:** The access at L1 targets address `r + 100`. This address does not fall within any allocated region in R. The only region is ρ₁ spanning `[r, r + 8)`. Since `r + 100 ∉ [r, r + 8)`, there is no region containing this address. The liveness invariant requires that every access targets an allocated region, and here no such region exists. □

### 3.4 Interpretation

**Claim:** Interpretation is violated.

**Proof:** The write `*(r + 100) = 42` attempts to write 8 bytes starting at offset 100 in region ρ₁. Since `100 + 8 = 108 > 8 = ρ₁.size`, the write exceeds the region boundary. There is no valid RepD for this access because it spans memory outside the allocated region. □

### 3.5 Exclusivity

**Claim:** Exclusivity holds trivially (single-threaded, sequential).

### 3.6 Cleanup

**Claim:** Cleanup is violated — region ρ₁ is never freed.

**Proof:** There is no `free(r)` in the program. Region ρ₁ is allocated but never deallocated, and there is no explicit annotation marking it as intentionally leaked. □

### 3.7 Verdict

**ORIGIN VIOLATED** (primary), **LIVENESS VIOLATED**, **INTERPRETATION VIOLATED**, **CLEANUP VIOLATED**. The program is **UNSAFE**.

**Counterexample path:** `L0 (alloc ρ₁, size 8) → L1 (write at r+100, offset 100 > size 8)`

---

## Program 4: Double Free

```
L0: r = allocate(8)
L1: free(r)
L2: free(r)       // VIOLATION
```

### 4.1 MSG Construction

**Regions:**

| ID  | Base | Size | Status     |
|-----|------|------|------------|
| ρ₁  | r    | 8    | Allocated  |

**Derivation edges:**

| Source   | Destination | Offset | Region |
|----------|-------------|--------|--------|
| (alloc)  | r           | 0      | ρ₁     |

**Access records:**

| Addr | Op    | Width | Program Point |
|------|-------|-------|---------------|
| r    | Free  | —     | L1            |
| r    | Free  | —     | L2            |

**Region state map S:**

| Program Point | ρ₁.status  |
|---------------|------------|
| L0 (after)    | Allocated  |
| L1 (after)    | Freed      |
| L2 (after)    | Freed      |

### 4.2 Cleanup — VIOLATION

**Claim:** The cleanup invariant is violated.

**Proof:** The cleanup invariant requires that every allocated region is freed *exactly once*. More precisely, for each region `ρ ∈ R`, there must exist exactly one access `a ∈ A` such that `a.op = Free` and `a.addr = ρ.base`. The uniqueness of the free operation is essential: a double free is as erroneous as a leak, because freeing already-freed memory corrupts the allocator's internal state and may cause undefined behavior.

In this program, the access set A contains two Free operations: one at L1 and one at L2, both targeting `r = ρ₁.base`. This means ρ₁ is freed twice. The cleanup invariant, properly stated, requires `|{ a ∈ A | a.op = Free ∧ a.addr = ρ.base }| = 1` for each region ρ. Here, the cardinality is 2, not 1. The invariant is violated.

**Counterexample execution path:** `L0 (alloc) → L1 (free ρ₁, status → Freed) → L2 (free ρ₁ again, but ρ₁.status = Freed)`. The second free at L2 operates on a region already in the Freed state. □

### 4.3 Liveness

**Claim:** The liveness invariant is also violated at L2.

**Proof:** The Free operation at L2 targets `r`, which lies within region ρ₁. At program point L2, `S(L2)(ρ₁) = Freed` because L1 already freed ρ₁. The liveness invariant requires that every access (including Free operations) targets an Allocated region. Since `S(L2)(ρ₁) = Freed`, liveness is violated. However, the more specific and informative violation is the cleanup invariant's uniqueness requirement, which directly captures the double-free semantics. □

### 4.4 Origin

**Claim:** Origin holds.

**Proof:** The address `r` traces back to `allocate(8)` at L0. Both free operations reference the same validly-derived address. The origin of the address is not in question — the error is in the *use* of the address, not its *provenance*. □

### 4.5 Exclusivity

**Claim:** Exclusivity holds trivially (single-threaded, sequential, no data accesses).

### 4.6 Interpretation

**Claim:** Interpretation is not applicable — Free operations do not interpret byte contents.

### 4.7 Verdict

**CLEANUP VIOLATED** (primary: double free), **LIVENESS VIOLATED** at L2. The program is **UNSAFE**.

**Counterexample path:** `L0 (alloc ρ₁) → L1 (free ρ₁, OK) → L2 (free ρ₁ again, ρ₁.status = Freed)`

---

## Program 5: Valid Pointer Arithmetic

```
L0: r = allocate(256)
L1: offset = r + 64
L2: *offset = 42    // Should be safe
```

### 5.1 MSG Construction

**Regions:**

| ID  | Base | Size | Status     |
|-----|------|------|------------|
| ρ₁  | r    | 256  | Allocated  |

**Derivation edges:**

| Source   | Destination | Offset | Region |
|----------|-------------|--------|--------|
| (alloc)  | r           | 0      | ρ₁     |
| r        | offset      | 64     | ρ₁     |

The second derivation edge records that `offset = r + 64` is derived from `r` with byte offset 64, within region ρ₁.

**Access records:**

| Addr    | Op    | Width | Program Point |
|---------|-------|-------|---------------|
| offset  | Write | 8     | L2            |

**Region state map S:**

| Program Point | ρ₁.status  |
|---------------|------------|
| L0 (after)    | Allocated  |
| L1 (after)    | Allocated  |
| L2 (after)    | Allocated  |

### 5.2 Origin Proof

**Claim:** The address `offset = r + 64` traces to a valid allocation and falls within bounds.

**Proof:** The derivation chain for `offset` is: `offset → r (offset 64) → ρ₁ (root)`. Region ρ₁ was created by `allocate(256)` at L0, so ρ₁ has size 256 bytes. We verify the bound constraint: `offset_within_region = 64`, `access_width = 8`. We require `64 + 8 ≤ 256`, which gives `72 ≤ 256`. This is true. Therefore the derived pointer `offset` references a valid location within ρ₁. **Origin holds.** □

### 5.3 Liveness Proof

**Claim:** The access at L2 targets a live region.

**Proof:** The access at L2 targets `offset`, which lies within region ρ₁ (as established by the derivation chain). At program point L2, `S(L2)(ρ₁) = Allocated`, because the allocation at L0 set this status and no free operation occurs before L2. Therefore the liveness invariant is satisfied. **Liveness holds.** □

### 5.4 Exclusivity Proof

**Claim:** No concurrent access conflict exists.

**Proof:** The program is single-threaded and sequential. There is only one data access (the Write at L2). No pair of concurrent accesses exists. By vacuous truth, **exclusivity holds.** □

### 5.5 Interpretation Proof

**Claim:** The write at L2 respects a valid representation descriptor.

**Proof:** The write `*offset = 42` stores a u64 value (8 bytes) at address `offset = r + 64`. We verify the byte range: `[r + 64, r + 64 + 8) = [r + 64, r + 72)`. This range is entirely contained within ρ₁, which spans `[r, r + 256)`, because `r + 72 ≤ r + 256`. The RepD at offset 64 is `u64` (8-byte unsigned integer), which is a valid interpretation for the write operation. The alignment constraint is also satisfied: assuming the allocator returns 8-byte-aligned addresses, `r` is 8-byte-aligned, and `r + 64` is also 8-byte-aligned (since 64 is a multiple of 8). **Interpretation holds.** □

### 5.6 Cleanup

**Claim:** Cleanup is violated — region ρ₁ is never freed.

**Proof:** There is no `free(r)` in the program. Region ρ₁ is allocated at L0 but never deallocated, and there is no intentional-leak annotation. However, this is a property of the *incomplete program*, not of the pointer arithmetic. In a complete program, ρ₁ would be freed at a later point. For the purposes of this proof, we note that cleanup is not satisfied as written, but the pointer arithmetic itself is safe. If we consider this a program fragment (not a complete program), cleanup is pending; if a complete program, cleanup is violated. □

### 5.7 Verdict

**Liveness holds. Exclusivity holds. Interpretation holds. Origin holds.** Cleanup is pending/violated depending on completeness assumption. The pointer arithmetic pattern is **SAFE**.

---

## Program 6: Derived Pointer After Free

```
L0: r = allocate(256)
L1: offset = r + 64
L2: free(r)
L3: *offset = 42    // VIOLATION
```

### 6.1 MSG Construction

**Regions:**

| ID  | Base | Size | Status     |
|-----|------|------|------------|
| ρ₁  | r    | 256  | Allocated  |

**Derivation edges:**

| Source   | Destination | Offset | Region |
|----------|-------------|--------|--------|
| (alloc)  | r           | 0      | ρ₁     |
| r        | offset      | 64     | ρ₁     |

**Access records:**

| Addr    | Op    | Width | Program Point |
|---------|-------|-------|---------------|
| r       | Free  | —     | L2            |
| offset  | Write | 8     | L3            |

**Region state map S:**

| Program Point | ρ₁.status  |
|---------------|------------|
| L0 (after)    | Allocated  |
| L1 (after)    | Allocated  |
| L2 (after)    | Freed      |
| L3 (after)    | Freed      |

### 6.2 Liveness — VIOLATION

**Claim:** The liveness invariant is violated at L3.

**Proof:** The access at L3 targets `offset = r + 64`. By the derivation edge, `offset` is derived from `r` with offset 64 within region ρ₁. Therefore `offset ∈ [ρ₁.base, ρ₁.base + 256)`. At program point L3, the region state map gives `S(L3)(ρ₁) = Freed`, because the `free(r)` at L2 transitioned ρ₁'s status from Allocated to Freed. The liveness invariant requires `S(a.pp)(ρ) = Allocated` for every access. Since `S(L3)(ρ₁) = Freed`, the invariant is violated.

The key insight is that derived pointers inherit the liveness status of their root region. Even though `offset` is a distinct address from `r`, it is *semantically tied* to ρ₁ through the derivation chain. When ρ₁ is freed, all derived pointers into ρ₁ become dangling — they target a freed region, regardless of the specific offset used. This is the fundamental reason the VUMA MSG tracks derivation chains: to propagate region status changes to all derived pointers.

**Counterexample execution path:** `L0 (alloc ρ₁, 256 bytes) → L1 (derive offset = r + 64) → L2 (free ρ₁, status → Freed) → L3 (write *offset, but ρ₁.status = Freed)`. The derived pointer `offset` becomes dangling after the free of its root region. □

### 6.3 Origin

**Claim:** Origin holds.

**Proof:** The address `offset` traces back through the derivation chain: `offset → r (offset 64) → ρ₁ (root)`. The chain is valid; `offset` does derive from a valid allocation. Origin is about provenance, not current liveness. The address is well-originated even though its target region is now freed. □

### 6.4 Exclusivity

**Claim:** Exclusivity holds trivially (single-threaded, sequential).

### 6.5 Interpretation

**Claim:** Interpretation would be violated if we reached it.

**Proof:** Although the offset calculation (64 + 8 = 72 ≤ 256) would be valid if ρ₁ were live, the region is Freed at L3. A Freed region has no valid RepD. Therefore the interpretation check also fails. However, liveness is checked first and is the primary violation. □

### 6.6 Cleanup

**Claim:** Cleanup holds.

**Proof:** Region ρ₁ is freed at L2 (exactly once). The cleanup invariant is satisfied. □

### 6.7 Verdict

**LIVENESS VIOLATED** at L3 (derived pointer targets freed region). The program is **UNSAFE**.

**Counterexample path:** `L0 (alloc ρ₁) → L1 (derive offset from ρ₁) → L2 (free ρ₁) → L3 (write through dangling derived pointer)`

---

## Summary Table

| Program | Description | Liveness | Exclusivity | Interpretation | Origin | Cleanup | Verdict |
|---------|-------------|----------|-------------|----------------|--------|---------|---------|
| 1 | Simple Allocation | ✅ | ✅ | ✅ | ✅ | ✅ | **SAFE** |
| 2 | Use-After-Free | ❌ | ✅ | ❌* | ✅ | ✅ | **UNSAFE** |
| 3 | Out-of-Bounds | ❌ | ✅ | ❌ | ❌ | ❌ | **UNSAFE** |
| 4 | Double Free | ❌ | ✅ | N/A | ✅ | ❌ | **UNSAFE** |
| 5 | Valid Pointer Arith. | ✅ | ✅ | ✅ | ✅ | ⚠️ | **SAFE*** |
| 6 | Derived Ptr After Free | ❌ | ✅ | ❌* | ✅ | ✅ | **UNSAFE** |

**Footnotes:**

- \* Interpretation violations in Programs 2 and 6 are secondary to the liveness violation; they would fail independently but are detected after the liveness check.
- \* Program 5 cleanup is marked ⚠️ because the program as written does not free ρ₁; this is likely a program fragment. The pointer arithmetic pattern itself is safe.
- The primary violation for each unsafe program is highlighted in the individual proof sections.

---

## Appendix: Derivation Chain Propagation Rule

A key principle demonstrated by Program 6 is the **derivation chain propagation rule**: when a region ρ transitions from Allocated to Freed, all addresses in the derivation set `D_ρ = { addr | ∃ d ∈ D : root(d) = ρ }` become dangling. The liveness of any derived pointer is equivalent to the liveness of its root region. This rule is formalized as:

```
∀ a ∈ A, ∀ ρ ∈ R:
  a.addr ∈ D_ρ ⟹ (S(a.pp)(ρ) = Allocated)
```

where `a.addr ∈ D_ρ` means `a.addr` is derived from region ρ (i.e., there exists a derivation chain from `a.addr` to ρ).

This propagation rule is what distinguishes VUMA's global reasoning from the borrow checker's local analysis: the IVE tracks not just the immediate pointer but the entire derivation graph, enabling it to detect that `offset` is invalidated by `free(r)` even though `offset ≠ r`.
