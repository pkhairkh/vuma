# VUMA Invariant Proof: Doubly-Linked List

**Task ID:** W1-24  
**Author:** Agent W1-24, VUMA Project  
**Date:** 2026-03-04  
**Status:** Formal Proof — Complete  
**Reference:** *Beyond Human Syntax* (Proposal §3.6, §3.6.3)

---

## 1. Introduction

This document provides a rigorous, operation-by-operation proof that a circular doubly-linked list with sentinel satisfies all five VUMA memory invariants under the Verified-Unsafe Memory Access model. The doubly-linked list is the canonical data structure that Rust's borrow checker cannot verify without `unsafe` — making it the ideal showcase for VUMA's global verification approach.

The VUMA model defines five global invariants that the Inference and Verification Engine (IVE) must prove for every program:

1. **Liveness** — Every pointer dereference targets allocated memory.
2. **Exclusivity** — No two simultaneous mutable accesses target the same memory.
3. **Interpretation** — Every memory access respects a valid representation descriptor.
4. **Origin** — Every address traces back to a valid allocation.
5. **Cleanup** — Every allocation is eventually freed.

We prove each invariant for each operation of the doubly-linked list.

---

## 2. Data Structure Definition

### 2.1 Node Layout

```
Node = { prev: Address, next: Address, data: u64 }
```

**Representation Descriptor (RepD):**
- Size: 24 bytes (3 × 8-byte fields)
- Alignment: 8 bytes
- Field offsets: `prev` at +0, `next` at +8, `data` at +16

### 2.2 List Structure

```
List = { sentinel: Address }
```

The list uses a **circular sentinel** design:
- The sentinel node is a `Node` whose `data` field is unused.
- `sentinel.next` points to the first real node (or back to sentinel if empty).
- `sentinel.prev` points to the last real node (or back to sentinel if empty).
- An empty list has `sentinel.prev = sentinel` and `sentinel.next = sentinel`.

**Invariant (structural):** For every node `n` in the list, `n.next.prev = n` and `n.prev.next = n`. This holds for the sentinel as well.

### 2.3 Memory State Graph (MSG) Notation

We write `alloc(a, 24, 8)` to denote a 24-byte, 8-byte-aligned allocation at address `a`. We write `free(a)` to denote deallocation. We write `live(a)` to mean address `a` is within an allocated region at the current program point. We write `*(a + offset)` for a memory access at address `a` with the given byte offset.

---

## 3. Operation Proofs

### 3.1 Operation: `new_list()`

**Specification:** Allocate a sentinel node. Set `sentinel.prev = sentinel` and `sentinel.next = sentinel`. Return the list handle.

**Implementation:**
```
new_list() → List:
    s = allocate(24, 8)        // allocate Node-sized region
    s.prev = s                 // circular: points to self
    s.next = s                 // circular: points to self
    return List { sentinel: s }
```

#### 3.1.1 Liveness Proof

**Claim:** After `new_list()`, the sentinel is allocated and all pointer dereferences within the operation target live memory.

**Proof:**

- `s = allocate(24, 8)`: `s` is freshly allocated. By the semantics of `allocate`, `live(s)` holds after this line. The region `[s, s+24)` is allocated.
- `s.prev = s`: This is a write to `s + 0`. Since `live(s)` holds and `0 < 24`, this write targets live memory. ✓
- `s.next = s`: This is a write to `s + 8`. Since `live(s)` holds and `8 < 24`, this write targets live memory. ✓
- The return value contains `s`, which is live. ✓

**Postcondition:** `live(s)` holds, `s.prev = s`, `s.next = s`. The sentinel points to itself, forming a valid empty circular list.

**Conclusion:** Liveness holds for `new_list()`. ∎

#### 3.1.2 Exclusivity Proof

**Claim:** No two simultaneous mutable accesses target the same memory during `new_list()`.

**Proof:**

All operations are sequential and single-threaded. The writes occur in order:
1. Write to `s + 0` (`s.prev`)
2. Write to `s + 8` (`s.next`)

These are non-overlapping regions within the same allocation (offset 0 and offset 8, each 8 bytes wide). They are also sequential, not concurrent. In single-threaded execution, exclusivity is trivially satisfied because only one access occurs at any point in time.

**Conclusion:** Exclusivity holds for `new_list()`. ∎

#### 3.1.3 Interpretation Proof

**Claim:** Every memory access in `new_list()` interprets the target bytes according to a valid representation descriptor.

**Proof:**

- `allocate(24, 8)`: Allocates a region with the Node RepD (24 bytes, 8-byte alignment). The allocation itself creates no interpretation conflict.
- `s.prev = s`: Writes an `Address` (8-byte pointer) to offset 0. The Node RepD specifies `prev: Address @ 0`. Interpretation matches. ✓
- `s.next = s`: Writes an `Address` (8-byte pointer) to offset 8. The Node RepD specifies `next: Address @ 8`. Interpretation matches. ✓

**Conclusion:** Interpretation holds for `new_list()`. ∎

#### 3.1.4 Origin Proof

**Claim:** Every address used in `new_list()` traces back to a valid allocation.

**Proof:**

- `s` is the return value of `allocate(24, 8)`. This is a root allocation — the origin of `s` is the allocation site itself.
- `s.prev` and `s.next` are set to `s`, which has a valid origin (the allocation). So all stored pointers trace to a valid allocation.
- No pointer arithmetic or address computation is performed beyond the initial allocation.

**Conclusion:** Origin holds for `new_list()`. ∎

#### 3.1.5 Cleanup Proof

**Claim:** The allocation in `new_list()` will eventually be freed.

**Proof:**

This is an allocation-creating operation. Cleanup cannot be proven for `new_list()` in isolation — it depends on the caller eventually invoking `free_list()`. However, the IVE tracks this allocation as having no matching `free()` yet and records it as a **pending deallocation obligation**. When `free_list()` is later proven (§3.5), the IVE closes the obligation. The full proof of cleanup is established at the program level, not the operation level.

**Partial conclusion:** Cleanup is conditionally satisfied, pending proof of `free_list()`. ∎

---

### 3.2 Operation: `push_back(list, value)`

**Specification:** Insert a new node containing `value` between the last node and the sentinel.

**Implementation:**
```
push_back(list, value):
    last = list.sentinel.prev        // current last node
    n = allocate(24, 8)              // allocate new node
    n.data = value                   // set data
    n.prev = last                    // link backward to last
    n.next = list.sentinel           // link forward to sentinel
    last.next = n                    // link last forward to new node
    list.sentinel.prev = n           // link sentinel backward to new node
```

#### 3.2.1 Liveness Proof

**Claim:** After `push_back()`, all existing nodes remain allocated and the new node is allocated. All pointer dereferences target live memory.

**Proof:**

- `last = list.sentinel.prev`: Reads `list.sentinel + 0`. By the structural invariant, `list.sentinel` is live (allocated by `new_list()`). Reading from a live region is valid. `last` is then either the sentinel itself (empty list) or a previously pushed node (non-empty). By induction, all previously pushed nodes are still live. ✓
- `n = allocate(24, 8)`: `n` is freshly allocated. `live(n)` holds. ✓
- `n.data = value`: Write to `n + 16`. `live(n)` holds, `16 < 24`. ✓
- `n.prev = last`: Write to `n + 0`. `live(n)` holds. `last` is live (proven above). ✓
- `n.next = list.sentinel`: Write to `n + 8`. `live(n)` holds. `list.sentinel` is live. ✓
- `last.next = n`: Write to `last + 8`. `live(last)` holds (by induction from structural invariant). `8 < 24`. ✓
- `list.sentinel.prev = n`: Write to `list.sentinel + 0`. `live(list.sentinel)` holds. `0 < 24`. ✓

**Postcondition:** The new node `n` is linked between `last` and `sentinel`. All existing nodes are untouched (only `last.next` is modified, which does not affect the liveness of other nodes). The structural invariant is maintained: `n.prev.next = n` (since `last.next = n`), `n.next.prev = n` (since `sentinel.prev = n`), and the old `last.next.prev` link (which was `last`) remains `last` because `n.prev = last`.

**Conclusion:** Liveness holds for `push_back()`. ∎

#### 3.2.2 Exclusivity Proof

**Claim:** No two simultaneous mutable accesses target the same memory during `push_back()`.

**Proof:**

The writes in `push_back()` target four distinct nodes:
1. `n.data` — write to node `n` at offset 16
2. `n.prev` — write to node `n` at offset 0
3. `n.next` — write to node `n` at offset 8
4. `last.next` — write to node `last` at offset 8
5. `list.sentinel.prev` — write to node `list.sentinel` at offset 0

**Case 1: List has at least one element (last ≠ sentinel).**

Nodes `n`, `last`, and `sentinel` are all distinct. Writes 1–3 target `n`, write 4 targets `last`, write 5 targets `sentinel`. No two writes target the same node. Since all operations are sequential, no conflicts arise. ✓

**Case 2: List is empty (last = sentinel).**

Writes 4 and 5 both target the sentinel node. Write 4 sets `sentinel.next` (offset 8), write 5 sets `sentinel.prev` (offset 0). These are **non-overlapping regions** within the same node (offset 0, size 8 vs. offset 8, size 8). Furthermore, they are **sequential**, not concurrent. Even within a single node, sequential writes to non-overlapping fields do not violate exclusivity.

**Conclusion:** Exclusivity holds for `push_back()` in all cases. ∎

#### 3.2.3 Interpretation Proof

**Claim:** Every memory access in `push_back()` respects the Node RepD.

**Proof:**

All reads and writes access fields at their declared offsets:
- `list.sentinel.prev`: read `Address @ +0` from a Node. ✓
- `n.data`: write `u64 @ +16` to a Node. ✓
- `n.prev`: write `Address @ +0` to a Node. ✓
- `n.next`: write `Address @ +8` to a Node. ✓
- `last.next`: write `Address @ +8` to a Node. ✓
- `list.sentinel.prev`: write `Address @ +0` to a Node. ✓

All interpretations match the Node RepD (size 24, align 8, fields at offsets 0, 8, 16).

**Conclusion:** Interpretation holds for `push_back()`. ∎

#### 3.2.4 Origin Proof

**Claim:** Every address used in `push_back()` traces to a valid allocation.

**Proof:**

- `list.sentinel`: Origin is the `allocate` in `new_list()`. ✓
- `last = list.sentinel.prev`: `last` is read from a pointer stored in the sentinel. By the structural invariant and induction, every `.prev`/`.next` pointer in the list points to a node that was allocated by a prior `push_back()` or `push_front()` call. Each such allocation is a valid origin. ✓
- `n = allocate(24, 8)`: Root allocation. ✓
- All stored pointers (`n.prev`, `n.next`) are set to `last` or `list.sentinel`, both of which have valid origins. ✓

Pointer derivation chain: `list.sentinel` → (read `.prev`) → `last`. This is a one-hop derivation: read an `Address` field from a live node. Since the field stores a valid address (by the structural invariant), the derived pointer has a valid origin.

**Conclusion:** Origin holds for `push_back()`. ∎

#### 3.2.5 Cleanup Proof

**Claim:** The allocation in `push_back()` will eventually be freed.

**Proof:**

Same argument as §3.1.5: the allocation of `n` creates a pending deallocation obligation. This obligation is discharged by `free_list()` (§3.5) or by `remove(n)` (§3.4). The IVE tracks this obligation globally.

**Partial conclusion:** Cleanup is conditionally satisfied. ∎

---

### 3.3 Operation: `push_front(list, value)`

**Specification:** Insert a new node containing `value` between the sentinel and the first node.

**Implementation:**
```
push_front(list, value):
    first = list.sentinel.next       // current first node
    n = allocate(24, 8)              // allocate new node
    n.data = value                   // set data
    n.prev = list.sentinel           // link backward to sentinel
    n.next = first                   // link forward to first
    first.prev = n                   // link first backward to new node
    list.sentinel.next = n           // link sentinel forward to new node
```

#### 3.3.1 Liveness Proof

**Claim:** After `push_front()`, all existing nodes remain allocated and the new node is allocated.

**Proof:** Symmetric to §3.2.1. The key steps:
- `first = list.sentinel.next`: `list.sentinel` is live; by the structural invariant, `first` is live. ✓
- `n = allocate(24, 8)`: Fresh allocation, `live(n)` holds. ✓
- All writes (`n.data`, `n.prev`, `n.next`, `first.prev`, `list.sentinel.next`) target live nodes at valid offsets. ✓

**Conclusion:** Liveness holds for `push_front()`. ∎

#### 3.3.2 Exclusivity Proof

**Claim:** No two simultaneous mutable accesses target the same memory during `push_front()`.

**Proof:** Symmetric to §3.2.2. The writes target `n` (three fields), `first` (one field), and `sentinel` (one field). In the empty-list case (`first = sentinel`), writes to `first.prev` and `list.sentinel.next` target the same node at offsets 0 and 8 — non-overlapping and sequential. In the non-empty case, all three nodes are distinct.

**Conclusion:** Exclusivity holds for `push_front()`. ∎

#### 3.3.3 Interpretation Proof

**Claim:** Every access respects the Node RepD.

**Proof:** Identical structure to §3.2.3. All field accesses match declared offsets and types. ✓

**Conclusion:** Interpretation holds for `push_front()`. ∎

#### 3.3.4 Origin Proof

**Claim:** Every address traces to a valid allocation.

**Proof:** Identical structure to §3.2.4. All pointers derive from `allocate()` or from `.prev`/`.next` reads of live nodes. ✓

**Conclusion:** Origin holds for `push_front()`. ∎

#### 3.3.5 Cleanup Proof

**Claim:** The allocation will eventually be freed.

**Proof:** Same as §3.2.5. Pending obligation discharged by `free_list()` or `remove()`. ✓

---

### 3.4 Operation: `remove(node)`

**Specification:** Unlink `node` from its list and free it. The node must be a real node (not the sentinel).

**Implementation:**
```
remove(node):
    prev = node.prev                 // node's predecessor
    next = node.next                 // node's successor
    prev.next = next                 // unlink: predecessor skips over node
    next.prev = prev                 // unlink: successor skips over node
    free(node)                       // deallocate the removed node
```

**This is the critical operation.** It is the one Rust's borrow checker cannot verify, because it involves simultaneous mutable access to adjacent nodes.

#### 3.4.1 Liveness Proof

**Claim:** After `remove(node)`, the removed node is freed, and all pointers to it were updated before the free. No use-after-free occurs.

**Proof:**

- `prev = node.prev`: Read from `node + 0`. `live(node)` holds (node has not been freed yet). By the structural invariant, `prev` is a live node. ✓
- `next = node.next`: Read from `node + 8`. `live(node)` holds. By the structural invariant, `next` is a live node. ✓
- `prev.next = next`: Write to `prev + 8`. `live(prev)` holds. This overwrites the pointer that previously pointed to `node`, replacing it with `next`. After this line, `prev` no longer points to `node`. ✓
- `next.prev = prev`: Write to `next + 0`. `live(next)` holds. This overwrites the pointer that previously pointed to `node`, replacing it with `prev`. After this line, `next` no longer points to `node`. ✓
- `free(node)`: Deallocates `node`. After this line, `live(node)` is **false**.

**Critical analysis: Are there any dangling pointers after `free(node)`?**

Before `free(node)`, there were exactly two pointers to `node`: `prev.next` and `next.prev`. Both were overwritten in lines 3 and 4 — **before** the `free()` call. Therefore, at the point of `free(node)`, no live pointer in the list targets `node`. The only remaining reference to `node` is the local variable `node` itself, which goes out of scope after `free()`. The `node` variable is never dereferenced after `free()`.

**This is the key insight that Rust's borrow checker cannot prove.** Rust sees `(*node.prev).next = node.next` and `(*node.next).prev = node.prev` as two mutable borrows that might alias (when `node.prev == node.next`, i.e., the single-element case). But VUMA's global analysis sees that:
1. Both borrows are to **adjacent** nodes, not the same node (in the general case).
2. Even when they happen to be the same node (sentinel in the single-element case), the writes target **different fields** at different offsets, and are **sequential**.
3. The borrows are completed (the writes finish) before `free()` is called, so no use-after-free is possible.

**Conclusion:** Liveness holds for `remove()`. ∎

#### 3.4.2 Exclusivity Proof

**Claim:** No two simultaneous mutable accesses target the same memory during `remove()`.

**Proof:**

The writes in `remove()` are:
1. `prev.next = next` — write to `prev + 8`
2. `next.prev = prev` — write to `next + 0`

**Case 1: `prev ≠ next` (list has ≥ 2 elements).**

The two writes target different nodes at different addresses. No aliasing. Exclusivity holds trivially. ✓

**Case 2: `prev = next` (list has exactly 1 element, so both point to sentinel).**

Both writes target the same node (the sentinel). However:
- Write 1 targets offset 8 (`next` field).
- Write 2 targets offset 0 (`prev` field).
- These are non-overlapping 8-byte regions within the same 24-byte node.
- The writes are **sequential** — write 1 completes before write 2 begins.

In VUMA, exclusivity is defined over **simultaneous** accesses. Sequential accesses to non-overlapping regions of the same allocation do not violate exclusivity, even if they occur within the same function invocation. This is because the memory model tracks accesses in program order, and two non-overlapping writes to the same allocation cannot interfere.

**Contrast with Rust's borrow checker:** Rust would reject this code because `(*node.prev).next = node.next` creates a mutable borrow of `*node.prev`, and `(*node.next).prev = node.prev` creates a mutable borrow of `*node.next`. When `node.prev == node.next` (both are the sentinel), these are two simultaneous mutable borrows of the same data, which Rust forbids — even though the borrows target different, non-overlapping fields and are used sequentially. Rust's local, field-insensitive analysis cannot distinguish "two mutable borrows of different fields of the same struct" from "two mutable borrows of the same data."

**Conclusion:** Exclusivity holds for `remove()` in all cases. ∎

#### 3.4.3 Interpretation Proof

**Claim:** Every memory access in `remove()` respects the Node RepD.

**Proof:**

- `node.prev`: read `Address @ +0` from a Node. ✓
- `node.next`: read `Address @ +8` from a Node. ✓
- `prev.next = next`: write `Address @ +8` to a Node. ✓
- `next.prev = prev`: write `Address @ +0` to a Node. ✓
- `free(node)`: deallocates a 24-byte, 8-byte-aligned Node region. ✓

All accesses match the Node RepD.

**Conclusion:** Interpretation holds for `remove()`. ∎

#### 3.4.4 Origin Proof

**Claim:** Every address used in `remove()` traces to a valid allocation.

**Proof:**

- `node`: The caller provides this address. By the precondition of `remove()`, `node` is a real node in the list, allocated by a prior `push_back()` or `push_front()`. Origin: the `allocate()` call in that push operation. ✓
- `prev = node.prev`: Derived by reading a pointer from a live node. By the structural invariant, this pointer points to a valid allocated node. Origin: the `allocate()` call that created that node. ✓
- `next = node.next`: Same argument as `prev`. ✓
- All stored values (`prev.next`, `next.prev`) are addresses (`next`, `prev`) with valid origins. ✓

Pointer derivation chains:
- `node.prev` → one-hop read from `node` → `prev` (origin: push allocation)
- `node.next` → one-hop read from `node` → `next` (origin: push allocation)

All derivations stay within allocated regions (they read an 8-byte pointer field from a 24-byte node, which is within bounds).

**Conclusion:** Origin holds for `remove()`. ∎

#### 3.4.5 Cleanup Proof

**Claim:** `free(node)` discharges the allocation obligation for `node`.

**Proof:**

The `free(node)` call exactly matches the `allocate(24, 8)` that created `node` in a prior push operation. The IVE verifies:
1. `node` was allocated exactly once (by a push operation).
2. `node` is freed exactly once (by this `remove()` or by `free_list()`).
3. No `free()` occurs twice for the same address.
4. After `free(node)`, no pointer to `node` is dereferenced (proven in §3.4.1).

This discharges the pending deallocation obligation for `node`.

**Conclusion:** Cleanup holds for `remove()`. ∎

---

### 3.5 Operation: `free_list(list)`

**Specification:** Iterate through the list, freeing every node, and free the sentinel last.

**Implementation:**
```
free_list(list):
    current = list.sentinel.next          // start at first node
    while current != list.sentinel:
        next = current.next               // save next pointer
        free(current)                     // free current node
        current = next                    // advance
    free(list.sentinel)                   // free sentinel last
```

#### 3.5.1 Liveness Proof

**Claim:** Every `free()` call targets a live node, and no freed node is accessed afterward.

**Proof:**

- `current = list.sentinel.next`: `list.sentinel` is live. Read from offset 8. By structural invariant, `current` is either `sentinel` (empty list) or a live node. ✓
- **Loop invariant:** At the start of each iteration, `current` is a live, allocated node that is not the sentinel.
- `next = current.next`: Read from `current + 8`. `current` is live (loop invariant). By structural invariant, `current.next` is a live node. ✓
- `free(current)`: Deallocates `current`. After this line, `live(current)` is false. But `current` is never dereferenced again — the next line sets `current = next`, which is a different, live node. ✓
- `current = next`: `next` was read from `current.next` before `current` was freed, so `next` is valid and live. ✓
- When the loop exits, `current = list.sentinel`, which is still live (it was never freed inside the loop). ✓
- `free(list.sentinel)`: Deallocates the sentinel. After this, the list is fully freed. ✓

**Critical point:** The loop saves `next = current.next` **before** freeing `current`. This ensures that the traversal pointer is not invalidated by the free. This is a classic pattern that VUMA's IVE can verify but that simpler local analyses sometimes miss.

**Conclusion:** Liveness holds for `free_list()`. ∎

#### 3.5.2 Exclusivity Proof

**Claim:** No two simultaneous mutable accesses occur.

**Proof:**

`free_list()` is single-threaded and sequential. Each `free()` call targets a different node (the loop advances before the next free). The `free()` calls are not simultaneous. No aliasing conflicts arise.

**Conclusion:** Exclusivity holds for `free_list()`. ∎

#### 3.5.3 Interpretation Proof

**Claim:** Every access respects the Node RepD.

**Proof:**

- `list.sentinel.next`: read `Address @ +8`. ✓
- `current.next`: read `Address @ +8`. ✓
- `free(current)`: deallocates a 24-byte, 8-byte-aligned region (matching the Node allocation). ✓
- `free(list.sentinel)`: same. ✓

All accesses match the Node RepD.

**Conclusion:** Interpretation holds for `free_list()`. ∎

#### 3.5.4 Origin Proof

**Claim:** Every address traces to a valid allocation.

**Proof:**

- `list.sentinel`: Origin is `allocate()` in `new_list()`. ✓
- `current`: Initially `list.sentinel.next` (one-hop derivation from sentinel). In subsequent iterations, `current = next = current_old.next` (one-hop derivation from the previously visited node). By induction, every value of `current` traces back through a chain of `.next` reads to the sentinel, which has a valid origin. ✓
- `next = current.next`: One-hop derivation from `current`. ✓

All derivation chains terminate at valid `allocate()` sites.

**Conclusion:** Origin holds for `free_list()`. ∎

#### 3.5.5 Cleanup Proof

**Claim:** Every node in the list is freed exactly once, and the sentinel is freed exactly once.

**Proof:**

The loop iterates through every real node between `sentinel.next` and `sentinel` (exclusive), freeing each one. By the structural invariant, these are exactly the nodes that were allocated by `push_back()` or `push_front()`. After the loop, `free(list.sentinel)` frees the sentinel allocated by `new_list()`.

**Total allocations:**
- 1 from `new_list()` (sentinel)
- N from `push_back()`/`push_front()` (N real nodes)

**Total deallocations:**
- N from the loop body (real nodes)
- 1 from `free(list.sentinel)` (sentinel)

Total allocations = total deallocations = N + 1. ✓

**No double-free:** Each node is visited exactly once by the loop (the list is acyclic in terms of real nodes — only the sentinel creates the cycle). The sentinel is freed after the loop exits. No node is freed twice.

**Combined with §3.4.5:** If some nodes were removed by `remove()` before `free_list()` is called, those nodes have already been freed. The loop will skip over them (they are no longer in the list). The allocation count still balances: each node is freed either by `remove()` or by `free_list()`, never both.

**Conclusion:** Cleanup holds for `free_list()`. ∎

---

## 4. Comparison with Rust's Borrow Checker

### 4.1 Why Rust Requires `unsafe`

Rust's borrow checker enforces memory safety through **local, syntactic rules**:

1. **Alias rule:** At any point, you may have either exactly one mutable reference (`&mut T`) or any number of immutable references (`&T`), but not both.
2. **Lifetime rule:** A reference must not outlive the data it borrows.
3. **Scope rule:** A borrow's lifetime is determined by its syntactic scope — the region of code where the reference is used.

These rules are **sound but incomplete**: they reject some safe programs. The doubly-linked list is the canonical example.

### 4.2 The Specific Conflict

Consider the `remove()` operation in Rust-like pseudocode:

```rust
fn remove(node: &mut Node) {
    let prev = node.prev;            // *mut Node
    let next = node.next;            // *mut Node
    unsafe {
        (*prev).next = next;         // mutable borrow of *prev
        (*next).prev = prev;         // mutable borrow of *next
    }
    free(node);
}
```

Rust requires `unsafe` here because `prev` and `next` are raw pointers (`*mut Node`), not references (`&mut Node`). The borrow checker **refuses to verify** raw pointer dereferences because it cannot apply its local rules to them.

But why can't we use references? Consider:

```rust
fn remove(node: &mut Node) {
    let prev = &mut *node.prev;      // ERROR: cannot borrow *node.prev as mutable
    let next = &mut *node.next;      // ERROR: cannot borrow *node.next as mutable
    prev.next = next;
    next.prev = prev;
}
```

This fails for two reasons:

1. **Self-referential borrows:** `node.prev` is an address stored in `node`. To create a reference to `*node.prev`, we need to read `node.prev` (which borrows `node`) while simultaneously creating a mutable reference to `*node.prev` (which borrows the previous node). The borrow checker sees two mutable borrows derived from overlapping data.

2. **Aliasing in the single-element case:** When the list has one element, `node.prev == node.next == sentinel`. Then `prev` and `next` would be two mutable references to the same data (the sentinel). Rust's alias rule forbids this categorically — it has no mechanism to determine that the two mutable references target **different fields** of the same struct and are used **sequentially**.

### 4.3 What Rust Cannot See

Rust's analysis is **field-insensitive** at the borrow level. When it sees `&mut *sentinel`, it considers the borrow to cover the entire `sentinel` struct, not just the specific field being accessed. This is a deliberate design choice: field-sensitive borrow checking would dramatically increase the complexity of the analysis and is not included in Rust's borrow checker.

More fundamentally, Rust's analysis is **local**: it examines each function in isolation (plus any lifetime annotations). It does not perform whole-program analysis to determine that:
- The sentinel is the only node where `prev` and `next` could point to the same node.
- Even in that case, the writes target different fields.
- The writes are sequential and do not conflict.

These facts require **global reasoning** — reasoning about the entire data structure and its invariants — which is beyond the scope of Rust's borrow checker by design.

### 4.4 What VUMA Can See

VUMA's IVE performs **global, field-sensitive, value-aware** analysis:

1. **Field-sensitive:** The IVE tracks which field of a struct is being accessed. It knows that `(*prev).next` writes to offset 8 and `(*next).prev` writes to offset 0. Even when `prev == next` (sentinel case), these are non-overlapping accesses.

2. **Value-aware:** The IVE tracks the actual values of pointers. It can determine that `prev == next` only in the single-element case, and that in all other cases the pointers are distinct. This enables it to prove exclusivity in both scenarios.

3. **Global:** The IVE reasons about the entire program's memory state. It can prove that the structural invariant (`n.next.prev = n` and `n.prev.next = n`) is maintained across all operations, which allows it to verify liveness, exclusivity, and cleanup across function boundaries.

4. **Sequentiality-aware:** The IVE understands that two mutable accesses to the same allocation are safe if they are sequential and non-overlapping. Rust's borrow checker conflates "multiple mutable borrows exist in the same scope" with "multiple mutable borrows are used simultaneously," rejecting code where the borrows are used sequentially.

### 4.5 The Fundamental Trade-off

Rust's design prioritizes **predictability and compilation speed**. The borrow checker's local rules are simple enough to explain to humans and fast enough to run on every compilation. The cost is that some safe programs (like doubly-linked lists) require `unsafe`.

VUMA's design prioritizes **expressiveness and verification completeness**. The IVE's global analysis can verify a strictly larger class of safe programs, including all programs that Rust can verify plus programs that Rust rejects (like doubly-linked lists). The cost is that verification is more computationally expensive and less predictable for humans — but since the primary consumer is an AI agent, not a human, this cost is acceptable.

The doubly-linked list proof above demonstrates this trade-off concretely: every operation satisfies all five VUMA invariants using raw, unrestricted pointer access, with no `unsafe` blocks, no borrow checker, and no runtime overhead. The safety is established entirely by the IVE's global reasoning.

---

## 5. Summary of Proof Results

| Operation    | Liveness | Exclusivity | Interpretation | Origin | Cleanup |
|-------------|----------|-------------|----------------|--------|---------|
| `new_list()`    | ✓        | ✓           | ✓              | ✓      | ✓*      |
| `push_back()`   | ✓        | ✓           | ✓              | ✓      | ✓*      |
| `push_front()`  | ✓        | ✓           | ✓              | ✓      | ✓*      |
| `remove()`      | ✓        | ✓           | ✓              | ✓      | ✓       |
| `free_list()`   | ✓        | ✓           | ✓              | ✓      | ✓       |

\* Cleanup for creation operations is conditionally satisfied, with the obligation discharged by `free_list()` or `remove()`.

**All five VUMA invariants hold for all operations of the doubly-linked list.** The list can be implemented with raw pointer access, no `unsafe` blocks, and no runtime safety checks — verified entirely by the IVE's global analysis.

---

## 6. Appendix: Formal Definitions of VUMA Invariants

For reference, the five VUMA invariants as defined in *Beyond Human Syntax* §3.6.2:

1. **Liveness Invariant:** ∀ access `*(a)` at program point `p`, `live(a)` holds at `p`. That is, the address `a` falls within a region that is allocated (not freed) at the point of access.

2. **Exclusivity Invariant:** ∀ pair of concurrent writes `*(a₁)` and `*(a₂)` at program point `p`, the regions `[a₁, a₁ + s₁)` and `[a₂, a₂ + s₂)` do not overlap. That is, no two simultaneous mutable accesses target overlapping memory.

3. **Interpretation Invariant:** ∀ access `*(a)` at program point `p`, the RepD of the operation matches the RepD of the allocated region at `a`. That is, the bytes are interpreted according to a valid representation descriptor.

4. **Origin Invariant:** ∀ address `a` used at program point `p`, there exists a derivation chain `a₀ → a₁ → ... → aₙ = a` where `a₀` is the return value of an `allocate()` call and each derivation step is either a field read (offset within an allocated region) or a pointer arithmetic operation that stays within bounds.

5. **Cleanup Invariant:** ∀ `allocate(r)` at program point `p`, there exists a `free(r)` at some program point `q` reachable from `p`, and no access to `r` occurs after `q`. That is, every allocation is eventually freed, and no freed region is accessed.

---

*End of proof document.*
