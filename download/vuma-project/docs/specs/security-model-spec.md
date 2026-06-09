# VUMA Security Model Specification

**Document ID:** VUMA-SPEC-SEC-001
**Version:** 1.1.0
**Status:** Formal Specification
**Authors:** Agent W1-30, VUMA Core Team
**Last Updated:** 2026-03-06

---

## Table of Contents

1. [Security Level Lattice](#1-security-level-lattice)
2. [Taint Tracking](#2-taint-tracking)
3. [Capability-Based Access Control](#3-capability-based-access-control)
4. [Security Boundary Enforcement](#4-security-boundary-enforcement)
5. [Attack Surface Reduction](#5-attack-surface-reduction)
6. [Pi 5 Specific Security](#6-pi-5-specific-security)
7. [IVE-Integrated Security Verification](#7-ive-integrated-security-verification)
8. [PAC Compliance Checking](#8-pac-compliance-checking)
9. [MTE Compliance Checking](#9-mte-compliance-checking)
10. [BTI Compliance Checking](#10-bti-compliance-checking)
11. [CapD→ARM64 PTE Mapping](#11-capdarm64-pte-mapping)
12. [Graduated Security Verdict](#12-graduated-security-verdict)

---

## 1. Security Level Lattice

### 1.1 Overview

VUMA enforces a mandatory access control (MAC) discipline based on a security level lattice. Every value that flows through the system carries, as part of its Relational Descriptor (RelD), a security classification drawn from a totally ordered set of levels. This classification governs all information flow: data may move from a lower level to an equal or higher level, but never downward. This no-downgrade rule is the foundational invariant of VUMA's information-flow security, and it is enforced statically by the Invariant Verification Engine (IVE) and dynamically by runtime checks emitted by the code generator.

### 1.2 Formal Definition

Let **L** denote the set of security levels:

```
L = { Public, Internal, Confidential, Secret, TopSecret }
```

The ordering relation <= on L is defined as:

```
Public <= Internal <= Confidential <= Secret <= TopSecret
```

This ordering is total: for any l1, l2 in L, exactly one of l1 < l2, l1 = l2, or l1 > l2 holds. The pair (L, <=) forms a lattice, equipped with two binary operations:

**Join** (least upper bound, written l1 v l2):

```
l1 v l2 = max(l1, l2)    under the ordering <=
```

The join returns the more restrictive (higher) of the two levels. When two pieces of data are combined — for example, by adding a Public integer to a Secret integer — the result inherits the join of their levels, which is Secret. This ensures that information can never be diluted by combination with less-sensitive data.

**Meet** (greatest lower bound, written l1 ^ l2):

```
l1 ^ l2 = min(l1, l2)    under the ordering <=
```

The meet returns the less restrictive (lower) of the two levels. The meet operation is used when determining the minimum clearance required to observe a pair of values simultaneously; if one value is Public and another is Confidential, the meet is Public, reflecting the fact that a Public-cleared observer can see at least one of them.

The lattice axioms are satisfied: for all a, b, c in L,

- **Commutativity:** a v b = b v a; a ^ b = b ^ a
- **Associativity:** (a v b) v c = a v (b v c); (a ^ b) ^ c = a ^ (b ^ c)
- **Absorption:** a v (a ^ b) = a; a ^ (a v b) = a
- **Idempotence:** a v a = a; a ^ a = a

The top element (T) is TopSecret, and the bottom element (bottom) is Public.

### 1.3 RelD Integration

Every value in VUMA carries its security level within its Relational Descriptor. Formally, the RelD of a value v includes a field:

```
SecurityRel {
    level: L,           // the security classification
    flow: FlowPolicy    // one of { FreeFlow, NoDowngrade, NoFlow }
}
```

The `level` field records the current security classification. The `flow` field encodes the permissible information-flow direction: `FreeFlow` allows unrestricted movement, `NoDowngrade` enforces the no-write-down rule (the default for most data), and `NoFlow` marks data as statically unmovable across certain boundaries (used for cryptographic key material, for example).

### 1.4 Information-Flow Rule

The central information-flow rule is:

```
For all values v_src with level l_src and v_dst with level l_dst:
    If information flows from v_src to v_dst, then l_src <= l_dst must hold.
```

A violation of this rule — an attempted flow from a higher level to a lower level — is classified as a **potential information leak**. The IVE detects such violations during static analysis by tracing DataFlow edges in the Semantic Connectivity Graph (SCG). If a violation is detected, the IVE emits a diagnostic and the compilation is aborted unless an explicit declassification annotation is present (see Section 4).

This rule applies to all channels: direct assignment, function return values, side effects (writes to shared memory, network sends), and implicit flows (control-flow dependencies). Implicit flows are tracked by observing that the security level of a value computed inside a conditional branch must be at least the join of the condition's level and the value's own level.

---

## 2. Taint Tracking

### 2.1 Overview

Taint tracking is a dynamic and static analysis technique that marks data originating from untrusted sources and propagates those marks through all computations that consume the tainted data. In VUMA, taint tracking is not an optional add-on or a separate tool — it is deeply integrated into the RelD and enforced by the IVE through the SCG's DataFlow edges. Every value in the system has a taint status that is part of its SecurityRel, and the IVE ensures that tainted data cannot influence security-critical decisions without explicit sanitization.

### 2.2 Sources of Taint

A value is considered tainted if it originates from any of the following sources:

1. **User Input:** Data read from stdin, GUI event handlers, command-line arguments, or environment variables. These are marked tainted at the point of creation because the user is an external entity whose inputs cannot be trusted by default.

2. **Network Packets:** Data received from any network socket — TCP, UDP, or any higher-level protocol. Network data is marked tainted regardless of the sender's claimed identity, because network traffic can be spoofed, intercepted, or manipulated by an attacker in a man-in-the-middle position.

3. **File Reads from Untrusted Paths:** Data read from filesystem paths that are writable by untrusted principals (e.g., /tmp, user home directories, or any path configured as untrusted in the deployment manifest). Reads from paths that are only writable by the process owner or by a trusted administrator are not tainted by default, but the taint policy can be overridden per deployment.

Formally, the taint source set S is defined as:

```
S = { UserInput, Network, UntrustedFile }
```

When a value v is created from a source s in S, its RelD is initialized with:

```
SecurityRel {
    level: Derived,          // derived from source context
    flow: NoDowngrade,
    taint: Tainted { source: s, sanitizable: true }
}
```

The `Derived` level means the security level is inferred from the context in which the value is used, combined with the taint source. A tainted value at a given explicit level is treated as if it were at the join of its explicit level and `Internal`, since tainted data should not be treated as Public even if the source classification would suggest it.

### 2.3 Taint Propagation

Taint propagation follows the principle that if a tainted value influences the computation of another value, the result inherits the taint. Formally:

```
If v_src is tainted with source s, and v_dst = f(v_src, ...) for some function f,
then v_dst is tainted with source s.
```

More generally, if multiple tainted values with sources s1, s2, ..., sn are used in a computation, the result is tainted with the union of those sources:

```
taint(v_dst) = { s1, s2, ..., sn }
```

The IVE implements taint propagation by walking the SCG's DataFlow edges. Each edge (v_src -> v_dst) carries the taint set of v_src forward to v_dst. If v_dst already has a taint set from another predecessor, the sets are unioned. This process is analogous to a fixed-point computation over the SCG.

Special cases:

- **Control-flow taint:** If a tainted value is used as a branch condition, all values assigned inside the branch are tainted, because an attacker who controls the condition can influence which branch is taken and therefore which values are produced. This is an **implicit flow** of information.
- **Container taint:** If any element of an array, struct, or map is tainted, the entire container is considered tainted. This conservative approximation prevents taint from being hidden inside data structures.
- **Pointer taint:** A pointer derived from a tainted address computation is tainted. Dereferencing a tainted pointer produces a tainted value, regardless of the pointed-to data's original taint status.

### 2.4 Taint Removal (Sanitization)

Taint can only be removed through explicit invocation of a verified sanitization function. A sanitization function is one that has been annotated with `#[sanitize(taint_source)]` and has been verified by the IVE to satisfy the following properties:

1. **Output independence:** The output of the sanitization function does not depend on the specific tainted input in a way that could leak information about the original tainted value. The IVE checks this by verifying that the function's output is within a fixed range or follows a fixed format, regardless of the input's content.

2. **No side channels:** The sanitization function does not leak information through timing, error messages, or other side channels. The IVE checks this by verifying that the function's execution time and error behavior are independent of the tainted input's value.

3. **Completeness:** The sanitization function handles all possible inputs from the taint source. It must not crash or produce undefined behavior on any input.

After sanitization, the value's RelD is updated:

```
SecurityRel {
    level: <original_level>,
    flow: NoDowngrade,
    taint: Clean
}
```

The security level is not changed by sanitization — only the taint status is cleared. This is because sanitization removes the risk of injection attacks but does not change the data's confidentiality classification.

### 2.5 IVE Integration

The IVE tracks taint through the SCG's DataFlow edges. At each node in the SCG, the IVE maintains a taint set. During the fixed-point computation, the IVE propagates taint sets along DataFlow edges. If a value with a non-empty taint set flows to a sink that requires a clean value (e.g., a system call, a database write, or an eval-like construct), the IVE emits a diagnostic error.

The SCG DataFlow edges used for taint tracking are:

```
enum DataFlowEdge {
    Direct { from: NodeId, to: NodeId },           // direct assignment or return
    Implicit { from: NodeId, to: NodeId },          // control-flow dependency
    Container { from: NodeId, container: NodeId },  // element -> container taint
    Pointer { from: NodeId, deref: NodeId },        // address -> value taint
}
```

---

## 3. Capability-Based Access Control

### 3.1 Overview

VUMA employs a capability-based access control model in which every value is associated with a Capability Descriptor (CapD) that enumerates the operations permitted on that value. Unlike traditional access control lists (ACLs) that are attached to objects and checked by a reference monitor, VUMA capabilities are carried with the value itself — they are part of the value's metadata — and are enforced by the IVE at compile time and by runtime checks emitted by the code generator. This design ensures that access control decisions are local, explicit, and cannot be bypassed by forgetting to check a permission.

### 3.2 Capability Set Definition

The CapD for a value v is a subset of the universal capability set C:

```
C = { Read, Write, Send, Execute, DerivePtr }
```

Each capability governs a specific class of operations:

- **Read:** Permits observation of the value's content. Without Read, the value's bits cannot be inspected, logged, or compared. The value can still be moved, stored, or passed as an argument, but its content is opaque to the holder.
- **Write:** Permits modification of the value's content. Without Write, the value is immutable. This includes both direct mutation (e.g., assignment to a field) and indirect mutation (e.g., passing the value to a function that writes through a pointer to it).
- **Send:** Permits transmission of the value over a network interface. Without Send, the value cannot be serialized and sent over any socket, pipe, or other communication channel that leaves the process's address space.
- **Execute:** Permits the value to be called or jumped to as code. Without Execute, the value cannot be used as a function pointer, a closure, or a return address. This is the critical capability that prevents code injection attacks: data values never carry Execute, so even if an attacker corrupts a data pointer, the corrupted pointer cannot be executed.
- **DerivePtr:** Permits the creation of a pointer to the value. Without DerivePtr, no address-of operation can be applied to the value. This prevents the creation of aliases that could be used to violate the value's access control policy.

Formally, a CapD is represented as:

```
CapD = { c in C | c is permitted for this value }
```

The default CapD for a newly created value is `{ Read, Write, DerivePtr }`. The `Send` and `Execute` capabilities are never granted by default and must be explicitly requested.

### 3.3 Capability Enforcement Rules

The IVE enforces the following rules:

**R1 (Read enforcement):** If an operation O requires observing the content of value v, then `Read in CapD(v)` must hold. Operations that require Read include: comparison operators, arithmetic operators (which inspect operands to produce results), print/display, and any function that reads from v. Violation: compile-time error, "value v cannot be read: missing Read capability."

**R2 (Write enforcement):** If an operation O requires modifying the content of value v, then `Write in CapD(v)` must hold. Operations that require Write include: assignment, field update, in-place mutation (e.g., increment), and any function that writes to v through a reference. Violation: compile-time error, "value v cannot be modified: missing Write capability."

**R3 (Send enforcement):** If an operation O requires transmitting value v over a network channel, then `Send in CapD(v)` must hold. Operations that require Send include: socket send, HTTP request body, IPC message, and any serialization that targets a remote endpoint. Violation: compile-time error, "value v cannot be transmitted: missing Send capability."

**R4 (Execute enforcement):** If an operation O requires calling value v as a function or jumping to v as code, then `Execute in CapD(v)` must hold. Operations that require Execute include: function pointer calls, closure invocations, and indirect jumps. Violation: compile-time error, "value v cannot be executed: missing Execute capability."

**R5 (DerivePtr enforcement):** If an operation O requires taking the address of value v or creating a pointer to v, then `DerivePtr in CapD(v)` must hold. Violation: compile-time error, "cannot derive pointer to v: missing DerivePtr capability."

### 3.4 Security Properties

The capability model eliminates entire classes of vulnerabilities by construction:

**Code injection prevention:** A value that arrives from an untrusted source (user input, network, untrusted file) is created with CapD = `{ Read }`. It lacks Execute, so it can never be called as code. Even if a buffer overflow were somehow possible (it is not, see Section 5), the overwritten function pointer would lack Execute and could not be invoked. This is a fundamental architectural guarantee: data and code are separated at the capability level, not just at the page-table level.

**Data exfiltration prevention:** A value that contains sensitive information (e.g., a cryptographic key, a password, personal data) is created with CapD = `{ Read, Write, DerivePtr }` and specifically excludes Send. This means the value can be used internally but can never be transmitted over a network. Even if a bug causes the value to be passed to a logging function that attempts to send data to a remote log aggregator, the Send check will fail at compile time (or at runtime for dynamic cases), preventing the exfiltration.

**Privilege escalation prevention:** Capabilities can only be reduced, never expanded. This is the **capability monotonicity rule**: if CapD(v) is the capability set at creation, then for any subsequent operation, CapD(v) can only become a subset of the original. There is no operation that adds a capability to a value. This prevents a compromised component from escalating its privileges by granting itself additional capabilities.

Formally:

```
For all v, for all times t1 < t2: CapD(v, t2) subset_eq CapD(v, t1)
```

---

## 4. Security Boundary Enforcement

### 4.1 Overview

A security boundary is a logical partition within the Semantic Connectivity Graph (SCG) that separates regions with different security postures. VUMA allows SCG Regions to be annotated as security boundaries, and the IVE enforces strict rules about how data and control flow can cross these boundaries. The security boundary model is the mechanism by which VUMA implements the principle of least privilege at the architectural level: each region operates with the minimum set of capabilities and the lowest security level consistent with its function, and any interaction between regions is mediated by explicit boundary-crossing checks.

### 4.2 Boundary Definition

An SCG Region R is a set of nodes in the SCG that are grouped together for security purposes. A security boundary B is a pair of adjacent regions:

```
B = (R_high, R_low)
```

where `level(R_high) > level(R_low)`, meaning the security classification of R_high is strictly greater than that of R_low. A boundary is marked by annotating a region with:

```
#[security_boundary(
    level: L,
    cross_permissions: { Read, Write, Send, Execute, DerivePtr },
    declassification_gate: Option<FunctionId>
)]
```

The `level` field specifies the security level of the region. The `cross_permissions` field specifies which capabilities are required for data to cross from this region to an adjacent region of lower level. The `declassification_gate` field, if present, names a verified sanitization function that can be used to downgrade data crossing the boundary.

### 4.3 Boundary Crossing Rules

**Rule B1 (Read-across):** If a value v with level l_v in region R_src is read by a node in region R_dst, and R_src and R_dst are separated by a security boundary B = (R_high, R_low), then:

- If R_src = R_high and R_dst = R_low: l_v must be <= level(R_low), or the read must go through the declassification gate. Otherwise, the IVE flags this as a potential information leak.
- If R_src = R_low and R_dst = R_high: the read is always permitted (information flows upward).

**Rule B2 (Write-across):** If a value v with level l_v in region R_dst is written by a node in region R_src, and R_src and R_dst are separated by a boundary:

- If R_src = R_high and R_dst = R_low: the write is a potential integrity violation (high-integrity data is being written into a low-integrity region). The IVE flags this unless the written data has been explicitly declassified.
- If R_src = R_low and R_dst = R_high: the write is a potential integrity violation (low-integrity data is being injected into a high-integrity region). The IVE flags this unless the CapD of the target value includes Write and the source value has been validated.

**Rule B3 (Control-flow across):** If control flows from a node in R_src to a node in R_dst across a boundary, then:

- The CapD of the calling function must include the capabilities specified in `cross_permissions` of the boundary.
- If R_src has a lower level than R_dst, the callee must not return data that would cause an implicit flow from R_dst to R_src unless the return value's level is <= level(R_src).

### 4.4 Declassification

Declassification is the controlled, explicit exception to the no-downgrade rule. It is required when a legitimate business need demands that high-level information be made available at a lower level (e.g., an audit log that must include a hash of secret data, or a user interface that must display a redacted version of confidential information).

A declassification operation is only valid if:

1. It is performed through a function that is annotated with `#[declassify(from_level, to_level)]`.
2. The declassification function has been verified by the IVE to produce output that is safe at the target level. This verification is analogous to sanitization verification: the function must not leak more information than is explicitly intended.
3. The declassification function is the designated `declassification_gate` for the boundary being crossed.

The RelD of a declassified value is updated as follows:

```
SecurityRel {
    level: to_level,                      // the new, lower level
    flow: NoDowngrade,
    taint: <original_taint>,              // taint is preserved
    declassified_by: FunctionId,          // provenance of the declassification
    declassified_at: SourceLocation       // where in the source code
}
```

The `declassified_by` and `declassified_at` fields provide an audit trail. Every declassification event is logged at runtime, enabling post-incident analysis of information leaks.

### 4.5 IVE Leak Detection

The IVE performs a whole-program analysis to detect potential information leaks across boundaries. The analysis proceeds as follows:

1. **Boundary identification:** The IVE scans the SCG for all region annotations that declare security boundaries.
2. **Edge classification:** For each DataFlow edge (v_src -> v_dst), the IVE determines whether the edge crosses a boundary by checking if v_src and v_dst belong to different regions that form a boundary pair.
3. **Level check:** For each crossing edge, the IVE checks the information-flow rule (l_src <= l_dst). If the rule is violated and no declassification gate is present on the edge, the IVE emits a **potential leak warning**.
4. **Implicit flow check:** The IVE also checks for implicit flows across boundaries by analyzing control-flow dependencies. If a branch condition in R_high influences the value of a variable in R_low, and the branch condition's level is > level(R_low), the IVE flags an implicit leak.

The leak detection is conservative: it may flag false positives (legitimate flows that appear to be leaks), but it will never miss a true positive (an actual leak). False positives can be resolved by adding explicit declassification annotations or by restructuring the code to eliminate the implicit flow.

---

## 5. Attack Surface Reduction

### 5.1 Overview

VUMA's design philosophy is that security should be a property of the language and its verification infrastructure, not a property of individual programs. By enforcing invariants at the language level, VUMA eliminates entire bug classes by construction, rather than relying on programmers to avoid them. The Invariant Verification Engine (IVE) is the mechanism that enforces these invariants: before any VUMA program is executed, the IVE verifies that the program satisfies a set of safety properties. If any property is violated, the program is rejected. This section catalogues the bug classes that VUMA eliminates and the invariants that enforce the elimination.

### 5.2 Buffer Overflows

**Bug class:** A buffer overflow occurs when a program writes beyond the bounds of an allocated buffer, corrupting adjacent memory. This is the most historically prevalent vulnerability class in systems programming languages.

**VUMA invariant (Bounds Invariant):** For every memory access `ptr[i]` or `ptr.offset(n)`, the IVE verifies that the accessed address lies within the allocated region of the pointer. The verification uses the RelD's allocation metadata, which records the base address and size of every allocation:

```
BoundsInvariant:
    For all ptr, i:
        base(ptr) <= addr(ptr, i) < base(ptr) + size(ptr)
```

The IVE checks this invariant by performing range analysis on the index expression. If the range of the index cannot be statically proven to lie within bounds, the IVE requires a runtime bounds check, which is automatically inserted by the code generator. The runtime check aborts the program on violation rather than allowing undefined behavior.

**Result:** Buffer overflows are impossible in VUMA. There is no way to write past the end of an array, because the IVE will either statically prove the access is safe or insert a runtime check that traps on violation.

### 5.3 Use-After-Free

**Bug class:** A use-after-free occurs when a program accesses memory that has been deallocated. The deallocated memory may have been reallocated for a different purpose, causing the stale pointer to read or write unintended data.

**VUMA invariant (Liveness Invariant):** For every pointer dereference `*ptr`, the IVE verifies that the allocation referenced by ptr has not been freed. The verification uses the SCG's lifetime analysis, which tracks the creation and destruction of every allocation:

```
LivenessInvariant:
    For all ptr, at time t:
        alloc(ptr) is live at time t
```

The IVE performs a borrow-check-like analysis (similar to Rust's borrow checker but operating on the SCG) to ensure that no pointer outlives its allocation. If a pointer escapes its allocation's lifetime, the IVE emits an error.

**Result:** Use-after-free is impossible in VUMA. Every pointer dereference is guaranteed to refer to live memory.

### 5.4 Double-Free

**Bug class:** A double-free occurs when a program frees the same allocation twice. This can corrupt the allocator's internal data structures and lead to arbitrary code execution.

**VUMA invariant (Cleanup Invariant):** For every deallocation operation `free(ptr)`, the IVE verifies that the allocation has not already been freed. The verification uses the SCG's ownership analysis, which ensures that each allocation has exactly one owner and that the owner performs exactly one cleanup:

```
CleanupInvariant:
    For all alloc a:
        count(free(a)) = 1
```

The IVE ensures this by requiring that the ownership of every allocation is transferred, not shared, and that the owner is responsible for cleanup. The ownership transfer is tracked through the SCG's DataFlow edges.

**Result:** Double-free is impossible in VUMA. Every allocation is freed exactly once by its unique owner.

### 5.5 Type Confusion

**Bug class:** Type confusion occurs when a program interprets a value of one type as a value of a different type, typically through an invalid cast. This can cause the program to interpret data as pointers, lengths, or function addresses, leading to arbitrary code execution.

**VUMA invariant (Interpretation Invariant):** For every value access, the IVE verifies that the value is interpreted according to its declared type. There are no unchecked casts in VUMA:

```
InterpretationInvariant:
    For all value v, interpretation I:
        I(v) is consistent with type(v)
```

All casts must be explicit and must go through a verified conversion function. The IVE checks that the conversion function produces a valid value of the target type for all possible inputs of the source type. Unions, if supported, are tagged and the tag is checked before access. There are no reinterpret casts or other mechanisms for bypassing the type system.

**Result:** Type confusion is impossible in VUMA. Every value is always interpreted according to its declared type, and no invalid cast can occur.

### 5.6 Code Injection

**Bug class:** Code injection occurs when an attacker supplies data that is interpreted as code by the program. This includes SQL injection, shell injection, and return-oriented programming (ROP).

**VUMA invariant (Execute Capability):** A value can only be executed if its CapD includes the Execute capability (see Section 3). Data values from untrusted sources are never granted Execute:

```
CodeInjectionPrevention:
    For all value v from source s in { UserInput, Network, UntrustedFile }:
        Execute not in CapD(v)
```

Since Execute can never be added to a value (capability monotonicity), tainted data can never become executable. This eliminates the entire class of injection attacks at the architectural level.

**Result:** Code injection is impossible in VUMA. No data from an untrusted source can ever be executed as code.

### 5.7 Data Races

**Bug class:** A data race occurs when two threads access the same memory location concurrently, at least one of the accesses is a write, and there is no synchronization between the accesses. Data races cause non-deterministic behavior and can lead to security vulnerabilities.

**VUMA invariant (Exclusivity Invariant):** For every write to a shared memory location, the IVE verifies that the writer has exclusive access. The verification uses the SCG's concurrency analysis, which tracks which threads have access to which memory locations at each point in the program:

```
ExclusivityInvariant:
    For all memory location m, at time t:
        |{ thread writing m at time t }| <= 1
        AND (if any thread writes m at time t, no other thread reads or writes m at time t)
```

The IVE enforces this by requiring that all shared mutable state be protected by synchronization primitives (mutexes, atomic operations) whose acquisition and release are tracked by the SCG. If two threads can access the same mutable location without synchronization, the IVE emits an error.

**Result:** Data races are impossible in VUMA. Every concurrent access to shared mutable state is guaranteed to be properly synchronized.

### 5.8 Remaining Attack Surface

The bug classes eliminated above are those that VUMA removes by construction. The remaining attack surface consists of:

1. **IVE bugs:** If the IVE itself has a bug that causes it to accept a program that violates an invariant, the security guarantees are void. Mitigation: the IVE is itself written in VUMA and verified, and its critical paths are kept small and well-tested. The IVE's proof obligations are documented and can be independently verified.

2. **Hardware vulnerabilities:** Speculative execution side channels (Spectre, Meltdown), rowhammer, and other hardware-level attacks are not prevented by VUMA's software-level invariants. Mitigation: VUMA's Pi 5 backend uses ARM64 PAC, BTI, and MTE to provide hardware-level defenses (see Section 6).

3. **Side channels:** Timing side channels, power analysis, and electromagnetic emanation are not prevented by VUMA's information-flow control, which operates at the logical level. Mitigation: VUMA's secret-aware code generation can insert timing-neutral operations to reduce timing side channels, but this is a best-effort mitigation, not a guarantee.

4. **Denial of service:** VUMA does not prevent resource exhaustion attacks. A program that allocates unbounded memory or enters an infinite loop can still deny service to other programs. Mitigation: deployment-level resource limits (cgroups, containers) are recommended.

---

## 6. Pi 5 Specific Security

### 6.1 Overview

The Raspberry Pi 5 is built on the Broadcom BCM2712 SoC, which features a quad-core ARM Cortex-A76 processor implementing the ARMv8.2-A architecture. This architecture includes several hardware security features that VUMA leverages to provide defense-in-depth beyond its software-level invariants. Specifically, VUMA maps its capability model to three ARM64 hardware security mechanisms: Pointer Authentication (PAC), Branch Target Identification (BTI), and the Memory Tagging Extension (MTE). These mappings create a layered defense where software invariants are backed by hardware enforcement, so that even if the IVE has a bug, the hardware provides a fallback.

### 6.2 ARM64 Pointer Authentication (PAC)

**ARM64 mechanism:** Pointer Authentication Codes (PAC) are cryptographic signatures that are embedded in the unused upper bits of 64-bit pointers. When a pointer is signed, a PAC is computed from the pointer value, a context value (typically the stack pointer or a function-specific key), and a secret key stored in a system register. When the pointer is used, the PAC is verified: if the signature does not match, the CPU generates an exception. This prevents pointer corruption, because an attacker who modifies a pointer without knowing the key cannot produce a valid PAC.

**VUMA mapping:** VUMA maps the DerivePtr capability to PAC signing. When a value v has `DerivePtr in CapD(v)`, the code generator emits a PAC signing instruction when the pointer is created:

```
// Pseudocode for CapD -> PAC mapping
fn create_pointer(v: Value) -> Ptr {
    if DerivePtr in CapD(v) {
        // Sign the pointer with the function's context key
        let signed_ptr = pac_sign(ptr_to(v), context=fp);
        return signed_ptr;
    } else {
        // Error: cannot derive pointer without DerivePtr capability
        compile_error!("missing DerivePtr capability");
    }
}
```

When the pointer is dereferenced, the code generator emits a PAC verification instruction:

```
fn dereference_pointer(p: Ptr) -> &Value {
    // Verify the PAC before dereferencing
    let verified_ptr = pac_verify(p, context=fp);
    // If verification fails, the CPU raises an exception
    return *verified_ptr;
}
```

This mapping ensures that every pointer in a VUMA program is authenticated by hardware. If the IVE incorrectly allows a pointer to be created without DerivePtr, the PAC mechanism provides a runtime check: the pointer will not have a valid signature, and any attempt to use it will trap.

**Key management:** VUMA uses two of the ARM64's five pointer authentication keys: APIAKey (for instruction addresses, used for function pointers) and APDAKey (for data addresses, used for data pointers). The keys are initialized early in the VUMA runtime startup and are never exposed to user code. The context value for each signature is the frame pointer of the enclosing function, which ensures that a signed pointer cannot be used in a different stack frame (preventing return-address forgery).

### 6.3 ARM64 Branch Target Identification (BTI)

**ARM64 mechanism:** Branch Target Identification (BTI) is a hardware mechanism that prevents indirect branches (jumps and calls through function pointers) from landing at arbitrary code locations. When BTI is enabled, each indirect branch must land on a special BTI instruction that specifies the type of branch that is permitted to land there (e.g., `bti c` for calls, `bti j` for jumps, `bti jc` for both). If an indirect branch lands on a non-BTI instruction or on a BTI instruction of the wrong type, the CPU generates an exception. This prevents Return-Oriented Programming (ROP) and Jump-Oriented Programming (JOP) attacks, which rely on chaining together sequences of instructions (gadgets) that end in indirect branches.

**VUMA mapping:** VUMA maps the Execute capability to BTI. When a function f has `Execute in CapD(f)`, the code generator marks the function's entry point with a BTI instruction:

```
// Pseudocode for CapD(Execute) -> BTI mapping
fn generate_function(f: Function) -> Assembly {
    if Execute in CapD(f) {
        // Emit BTI landing pad at function entry
        emit!("bti c");  // permit indirect calls
    } else {
        // No BTI instruction: indirect branches here will trap
        emit!("bti j");  // permit indirect jumps only (for non-callable code)
    }
    // ... rest of function body ...
}
```

For code pages that contain only data (no executable functions), the code generator does not emit any BTI instructions and marks the pages as non-executable in the page tables. This means that an attacker who manages to redirect execution to a data page will immediately trap, because the page is non-executable and contains no valid BTI landing pads.

**BTI and PAC interaction:** BTI and PAC work together to prevent ROP/JOP. PAC prevents the creation of forged function pointers (because the attacker cannot produce a valid PAC), and BTI prevents the use of valid-but-unintended function pointers as branch targets (because the target must have a matching BTI instruction). The combination ensures that every indirect branch goes to a known, intended target.

### 6.4 ARM64 Memory Tagging Extension (MTE)

**ARM64 mechanism:** The Memory Tagging Extension (MTE) provides hardware-assisted memory safety by associating a 4-bit tag with each 16-byte granule of physical memory. When a pointer is created, a tag is stored in the upper bits of the pointer. When the pointer is used to access memory, the CPU checks that the pointer's tag matches the tag of the accessed memory granule. If the tags do not match, the CPU generates an exception (synchronous tag check fault) or reports the violation asynchronously (asynchronous tag check fault). MTE provides probabilistic protection against spatial and temporal memory safety errors: the 4-bit tag provides 16 possible values, so the probability of a tag collision for an unrelated allocation is 1/16.

**VUMA mapping:** VUMA uses MTE as a defense-in-depth mechanism for the Bounds Invariant and Liveness Invariant. When an allocation is created, the VUMA runtime assigns a random MTE tag to the allocation's memory region and embeds the same tag in all pointers to that allocation:

```
// Pseudocode for MTE integration
fn allocate(size: usize) -> Ptr {
    let tag = random_4bit_tag();  // hardware random number
    let ptr = mte_alloc(size, tag);
    return ptr;  // pointer includes tag in upper bits
}

fn deallocate(ptr: Ptr) {
    let tag = extract_tag(ptr);
    mte_dealloc(ptr, tag);
    // After deallocation, the memory granule's tag is changed
    // to a different random value, causing any stale pointers
    // to fail the tag check on access
    mte_retag(ptr, random_4bit_tag());
}
```

This mapping provides runtime detection of two classes of bugs that the IVE is designed to prevent statically:

1. **Buffer overflows:** If the IVE incorrectly approves an out-of-bounds access, the pointer's tag will not match the tag of the overflowed granule (which belongs to a different allocation with a different tag), and the CPU will trap.

2. **Use-after-free:** After deallocation, the memory granule's tag is changed to a different random value. If a stale pointer is used, its tag will not match the new tag, and the CPU will trap.

**MTE mode selection:** VUMA uses MTE in synchronous mode (tag checks are performed immediately and faults are precise) during development and testing, and in asynchronous mode (tag checks are performed with a small delay and faults are imprecise) in production. Asynchronous mode has lower performance overhead but provides slightly weaker guarantees (a small window of execution may occur before the fault is reported). The mode is configurable at deployment time.

### 6.5 Comprehensive Mapping Table

| VUMA Concept         | ARM64 Feature | Mapping                                                |
|----------------------|---------------|--------------------------------------------------------|
| DerivePtr capability | PAC           | Pointer creation emits PAC sign; dereference emits verify |
| Execute capability   | BTI           | Function entries emit BTI landing pads; data pages marked non-executable |
| Bounds invariant     | MTE           | Allocation tags prevent spatial overflows              |
| Liveness invariant   | MTE           | Deallocation retagging prevents use-after-free         |
| Cleanup invariant    | MTE           | Double-free detected by tag mismatch on second free    |
| Capability monotonicity | PAC + BTI | PAC prevents forging new pointers; BTI prevents redirecting execution |
| No-downgrade flow    | All three     | Combined PAC+BTI+MTE prevents any bypass of flow control |

### 6.6 Performance Considerations

The performance overhead of the hardware security features is:

- **PAC:** Signing and verifying a pointer costs approximately 5-10 cycles per operation. Since VUMA signs pointers at creation and verifies at dereference, the overhead is proportional to the number of pointer operations, typically 1-3% for compute-intensive workloads.
- **BTI:** The BTI instruction is a single-cycle NOP on ARM Cortex-A76 when BTI is not enabled in the hardware, and a 1-cycle check when enabled. The overhead is negligible.
- **MTE:** MTE tag checks add approximately 1-2 cycles per memory access. The allocation and deallocation overhead for tag assignment and retagging is approximately 10-20 cycles per operation. The total overhead is typically 2-5% for memory-intensive workloads.

The combined overhead of PAC + BTI + MTE is typically 3-8% for most workloads on the Pi 5, which is acceptable for the security benefits provided.

---

## Appendix A: Formal Security Theorems

**Theorem 1 (Noninterference):** For any VUMA program P that passes IVE verification, if two executions of P differ only in their High-level inputs, then their Low-level outputs are identical.

*Proof sketch:* By the information-flow rule (Section 1.4), High-level data can only flow to High-level or higher outputs. The IVE verifies this by tracing all DataFlow edges in the SCG and checking that no edge leads from a High-level node to a Low-level node. Since taint propagation preserves the no-downgrade property (Section 2.3), and capabilities enforce the read/write/send/execute restrictions (Section 3), there is no channel by which High-level data can influence Low-level outputs.

**Theorem 2 (Capability Monotonicity):** For any VUMA value v, CapD(v) is a monotonically non-increasing function over the lifetime of v.

*Proof sketch:* The only operations that modify CapD are capability restriction operations, which remove capabilities from the set. There is no operation that adds a capability. This is enforced by the IVE, which rejects any program that attempts to expand CapD(v).

**Theorem 3 (Memory Safety):** For any VUMA program P that passes IVE verification, all memory accesses are within bounds, target live allocations, and are properly typed.

*Proof sketch:* The Bounds Invariant (Section 5.2), Liveness Invariant (Section 5.3), and Interpretation Invariant (Section 5.5) are verified by the IVE for every memory access in P. If any invariant is not statically provable, the code generator inserts a runtime check that traps on violation.

---

## Appendix B: Glossary

| Term    | Definition                                                        |
|---------|-------------------------------------------------------------------|
| CapD    | Capability Descriptor — the set of permitted operations on a value |
| RelD    | Relational Descriptor — metadata including security level and taint |
| SCG     | Semantic Connectivity Graph — the program's intermediate representation |
| IVE     | Invariant Verification Engine — the static verifier               |
| PAC     | Pointer Authentication Codes — ARM64 cryptographic pointer signing |
| BTI     | Branch Target Identification — ARM64 indirect branch validation   |
| MTE     | Memory Tagging Extension — ARM64 hardware memory safety           |
| MAC     | Mandatory Access Control — policy-driven access control           |
| ROP     | Return-Oriented Programming — an exploit technique                |
| JOP     | Jump-Oriented Programming — an exploit technique                  |

---

## 7. IVE-Integrated Security Verification

### 7.1 Overview

The IVE-Integrated Security Verification subsystem unifies VUMA's static invariant checking with the ARM64 hardware security features (PAC, MTE, BTI) into a single, coherent verification pass. While Sections 1–6 of this specification define the individual security mechanisms in isolation, this section describes how the IVE orchestrates them jointly through the `verify_security_properties()` function. This function is the top-level entry point invoked by the VUMA compiler after the SCG has been constructed and all individual BD inference passes have completed. It performs a whole-program security analysis that checks not only that each individual invariant holds, but also that the invariants are mutually consistent with the hardware enforcement mechanisms that will be applied at runtime. This cross-layer consistency check is essential because a program could, in principle, satisfy each invariant individually while still creating situations where the hardware enforcement conflicts with the software-level expectations — for example, a CapD that permits Execute but a PTE configuration that marks the corresponding page as non-executable.

### 7.2 `verify_security_properties()` Function

The `verify_security_properties()` function takes the fully constructed SCG, the inferred BD assignments for all nodes, and the target platform descriptor as inputs. It returns a `SecurityVerificationResult` that encapsulates the comprehensive security posture of the program:

```
struct SecurityVerificationResult {
    verdict: SecurityVerdict,               // See Section 12
    pac_result: PACComplianceResult,        // See Section 8
    mte_result: MTEComplianceResult,        // See Section 9
    bti_result: BTIComplianceResult,        // See Section 10
    pte_mapping_result: PTEMappingResult,   // See Section 11
    cross_layer_violations: Vec<CrossLayerViolation>,
    verified_properties: Vec<SecurityProperty>,
}
```

The function executes the following algorithm:

1. **Individual compliance checks:** Invoke `check_pac_compliance()`, `check_mte_compliance()`, and the BTI compliance checker on the entire SCG. Collect all violations from each checker.
2. **Cross-layer consistency analysis:** For each SCG node that involves both a capability-level and a hardware-level concern (e.g., a pointer dereference that must satisfy both CapD and PAC), verify that the CapD-derived expectations are consistent with the hardware enforcement that will be applied. Flag any `CrossLayerViolation` where the software model and hardware model disagree.
3. **PTE attribute synthesis:** Compute the ARM64 Page Table Entry attributes for each memory region using `capd_to_pte_attributes()` (see Section 11). Verify that the synthesized PTE attributes are consistent with the CapD assignments.
4. **Verdict computation:** Compute the overall `SecurityVerdict` based on the collected violations and the decision logic described in Section 12.
5. **Property cataloguing:** Enumerate the security properties that have been verified (e.g., "all pointers are PAC-signed", "all executable pages have BTI landing pads").

### 7.3 IVE–Hardware Integration Model

The integration between the IVE's invariant checking and the hardware security features operates on a principle of **defense-in-depth consistency**: the IVE's static guarantees must be a superset of the hardware's runtime guarantees. If the IVE proves that a pointer is never corrupted, the hardware PAC check must not contradict this by allowing an unsigned pointer to be used. Conversely, if the hardware provides a fallback (e.g., MTE detecting a buffer overflow that the IVE missed), the IVE's verdict should reflect that the program is partially but not fully secure.

The integration model defines three levels of consistency:

- **Strict consistency:** The IVE proof and the hardware enforcement agree exactly. The IVE proves that a property holds, and the hardware enforces it at runtime. Any runtime violation indicates an IVE bug.
- **Relaxed consistency:** The IVE cannot fully prove a property statically, so it inserts a runtime check that the hardware enforces. The hardware provides the primary defense, and the IVE's role is to minimize the number of such checks.
- **Inconsistency:** The IVE's static model and the hardware's enforcement model disagree. This is always a bug in VUMA itself and triggers a compiler-internal error.

---

## 8. PAC Compliance Checking

### 8.1 Overview

PAC Compliance Checking is the process by which the IVE verifies that every pointer derivation and usage in a VUMA program is compatible with the ARM64 Pointer Authentication mechanism. While Section 6.2 describes the PAC mechanism and its high-level mapping to VUMA's DerivePtr capability, this section provides the detailed compliance rules, the `check_pac_compliance()` function specification, and the `PACViolation` taxonomy. PAC compliance is critical because a pointer that bypasses PAC signing creates a gap in the defense-in-depth model: the IVE may believe the pointer is safe (based on CapD analysis), but the hardware cannot detect corruption of that pointer at runtime.

### 8.2 `check_pac_compliance()` Function

The `check_pac_compliance()` function traverses all pointer-related SCG nodes and edges, checking each one against the PAC compliance rules. Its signature is:

```
fn check_pac_compliance(
    scg: &SemanticConnectivityGraph,
    bd_assignments: &HashMap<NodeId, BD>,
    platform: &PlatformDescriptor,
) -> PACComplianceResult
```

The `PACComplianceResult` contains:

```
struct PACComplianceResult {
    is_compliant: bool,
    violations: Vec<PACViolation>,
    signed_pointers: Vec<NodeId>,      // pointers that are correctly PAC-signed
    unsigned_pointers: Vec<NodeId>,    // pointers that intentionally lack PAC (e.g., immediate offsets)
}
```

The algorithm processes each SCG node that involves a pointer value. For each such node, it checks whether the pointer's CapD includes DerivePtr (which triggers PAC signing at code generation time) and whether the pointer is used in a way that is consistent with PAC's constraints.

### 8.3 PAC Violation Taxonomy

A `PACViolation` represents a specific way in which a pointer derivation or usage is incompatible with PAC enforcement. The violation kinds are:

```
enum PACViolationKind {
    // A pointer was derived through arithmetic on a signed pointer without
    // first stripping the PAC. This produces an invalid PAC on the result.
    ArithmeticOnSignedPointer,

    // A pointer with DerivePtr in its CapD was not signed at creation.
    // This means the hardware cannot detect corruption of this pointer.
    MissingSignature,

    // A signed pointer was used without verification before dereference.
    // This means the hardware check is bypassed.
    MissingVerification,

    // A pointer's PAC context does not match the usage context.
    // E.g., a pointer signed with one function's FP is used in another function.
    ContextMismatch,

    // A signed pointer was stored to or loaded from a location that
    // does not preserve PAC bits (e.g., a truncated 32-bit store).
    PACBitTruncation,
}

struct PACViolation {
    kind: PACViolationKind,
    node_id: NodeId,
    pointer_bd: BD,
    description: String,
}
```

### 8.4 PAC Compliance Rules

The IVE enforces the following PAC compliance rules. Each rule corresponds to a specific violation kind:

**Rule PAC-1 (DerivePtr → Sign):** If a pointer node has `DerivePtr in CapD(node)`, then the code generator must emit a PAC signing instruction (`PACIx` or `PACDx`) at the point of pointer creation. If the IVE cannot confirm that the code generator will emit this instruction (e.g., because the pointer is created through a code path that the IVE cannot analyze), the IVE flags a `MissingSignature` violation.

**Rule PAC-2 (Dereference → Verify):** If a pointer is dereferenced, the code generator must emit a PAC verification instruction (`AUTIx` or `AUTDx`) before the load or store. If the IVE cannot confirm this, it flags a `MissingVerification` violation. The only exception is for pointers that are known to be immediately derived (e.g., `sp + offset` for stack locals), which are exempt from PAC signing and verification by construction.

**Rule PAC-3 (No arithmetic on signed pointers):** If a pointer is signed (i.e., it has a PAC embedded in its upper bits), then no arithmetic operation may be performed on the pointer value without first stripping the PAC. Performing arithmetic on a signed pointer corrupts the PAC, making the signature invalid. If the IVE detects such an operation, it flags an `ArithmeticOnSignedPointer` violation. The correct pattern is to strip the PAC (`XPACI`/`XPACD`), perform the arithmetic, and re-sign the result.

**Rule PAC-4 (Context consistency):** A pointer signed with context C must be verified with the same context C. If the IVE detects that a pointer is signed in one function (context = FP of function A) but verified in another function (context = FP of function B), it flags a `ContextMismatch` violation. Cross-function pointer passing requires using a stable context (e.g., the pointer's own address, or a dedicated context value agreed upon by the caller and callee).

**Rule PAC-5 (PAC bit preservation):** When a signed pointer is stored to memory or copied to another register, the full 64-bit value must be preserved. Storing only the lower 32 bits of a signed pointer truncates the PAC and renders it useless. The IVE flags a `PACBitTruncation` violation if it detects a 32-bit store of a signed 64-bit pointer.

---

## 9. MTE Compliance Checking

### 9.1 Overview

MTE Compliance Checking verifies that VUMA's memory operations are compatible with the ARM64 Memory Tagging Extension. While Section 6.4 describes the MTE mechanism and its mapping to VUMA's bounds and liveness invariants, this section provides the detailed compliance rules, the `check_mte_compliance()` function specification, and the `MTEViolation` taxonomy. MTE compliance ensures that every pointer carries a valid tag, that tags are correctly propagated through pointer arithmetic, and that tag changes during deallocation are properly sequenced relative to subsequent accesses. Without MTE compliance checking, a program could create pointers that lack tags (bypassing the hardware check) or could access memory after a tag change without observing the new tag (a TOCTOU vulnerability in the tagging mechanism).

### 9.2 `check_mte_compliance()` Function

The `check_mte_compliance()` function traverses all memory-related SCG nodes and checks each against the MTE compliance rules. Its signature is:

```
fn check_mte_compliance(
    scg: &SemanticConnectivityGraph,
    bd_assignments: &HashMap<NodeId, BD>,
    platform: &PlatformDescriptor,
) -> MTEComplianceResult
```

The `MTEComplianceResult` contains:

```
struct MTEComplianceResult {
    is_compliant: bool,
    violations: Vec<MTEViolation>,
    tagged_allocations: Vec<NodeId>,      // allocations with valid MTE tags
    untagged_allocations: Vec<NodeId>,    // allocations intentionally exempt from MTE
}
```

### 9.3 MTE Violation Taxonomy

An `MTEViolation` represents a specific way in which a memory operation is incompatible with MTE enforcement:

```
enum MTEViolationKind {
    // A pointer was created for an MTE-tagged allocation but the
    // pointer does not carry the allocation's tag. This means the
    // hardware cannot detect misuse of this pointer.
    MissingTag,

    // Pointer arithmetic produced an address that crosses a tag
    // granule boundary. The resulting pointer's tag applies to a
    // different 16-byte granule than intended, causing a false
    // positive or false negative tag check.
    CrossGranuleArithmetic,

    // A memory access occurs after the allocation's tag has been
    // changed (e.g., after deallocation and retagging), but the
    // access uses the old tag. This is the MTE-specific form of
    // use-after-free.
    StaleTag,

    // An allocation was created without a tag on a platform that
    // requires MTE. This means the allocation has no hardware
    // memory safety protection.
    UntaggedAllocation,

    // A deallocation did not change the memory's tag, meaning
    // stale pointers can still pass the tag check (a 1/16 chance
    // per allocation that the old and new tags collide).
    MissingRetag,
}

struct MTEViolation {
    kind: MTEViolationKind,
    node_id: NodeId,
    allocation_id: AllocationId,
    description: String,
}
```

### 9.4 MTE Compliance Rules

**Rule MTE-1 (Allocation → Tag):** Every heap allocation on an MTE-capable platform must be assigned a random 4-bit tag. The tag must be embedded in every pointer derived from the allocation's base address. If the IVE detects an allocation without a tag assignment, it flags an `UntaggedAllocation` violation.

**Rule MTE-2 (Pointer derivation → Tag propagation):** When a pointer is derived from a tagged base pointer through offset addition (e.g., `base + 8` for a field access), the derived pointer must carry the same tag as the base pointer, provided the offset does not cross a 16-byte granule boundary. If the offset does cross a boundary, the derived pointer's tag may differ from the base's tag, which is correct behavior — but the IVE must verify that the derived pointer is used only to access the granule it actually addresses. If the IVE cannot verify this, it flags a `CrossGranuleArithmetic` violation.

**Rule MTE-3 (Deallocation → Retag):** When a heap allocation is freed, the VUMA runtime must assign a new random tag to the deallocated memory granules. If the IVE detects a deallocation path that does not include a retagging step, it flags a `MissingRetag` violation. The retagging is the primary defense against use-after-free under MTE: without it, the stale pointer's tag would still match the memory's tag, and the hardware would not detect the illegal access.

**Rule MTE-4 (Access ordering):** All memory accesses to a given allocation must occur before the deallocation's retagging step. If the IVE detects a memory access that may occur after retagging (i.e., there exists a control-flow path from the retagging to the access), it flags a `StaleTag` violation. This is analogous to the Liveness Invariant (Section 5.3) but is specific to the MTE tag lifecycle.

**Rule MTE-5 (Tag consistency):** Every pointer that is stored to memory or passed across a function call must carry the correct tag for its target allocation. If the IVE detects a pointer that has been stripped of its tag (e.g., by a bit manipulation that clears the upper nibble), it flags a `MissingTag` violation. Stripping a tag is only permissible for immediate offset calculations that re-add the tag before use.

---

## 10. BTI Compliance Checking

### 10.1 Overview

BTI Compliance Checking verifies that VUMA's control-flow graph is compatible with the ARM64 Branch Target Identification mechanism. While Section 6.3 describes the BTI mechanism and its mapping to VUMA's Execute capability, this section provides the detailed compliance rules and the `BTIViolation` taxonomy. BTI compliance ensures that every indirect branch target has a valid BTI landing pad, that the BTI instruction type matches the branch type (call vs. jump), and that no executable page lacks BTI protection. Without BTI compliance checking, a program could contain indirect branch targets that lack BTI instructions, creating gadgets that an attacker could chain in a ROP or JOP attack.

### 10.2 BTI Compliance Checking Algorithm

The BTI compliance checker traverses all control-flow edges in the SCG, identifying every indirect branch (i.e., every edge whose source is a ControlNode with an indirect target). For each such edge, the checker verifies that the target node's generated code will begin with a BTI instruction of the appropriate type. The checker also verifies that no executable page is populated with code that lacks BTI landing pads at all indirect branch targets.

```
struct BTIComplianceResult {
    is_compliant: bool,
    violations: Vec<BTIViolation>,
    protected_targets: Vec<NodeId>,      // indirect branch targets with correct BTI
    unprotected_targets: Vec<NodeId>,    // indirect branch targets without BTI
}
```

### 10.3 BTI Violation Taxonomy

A `BTIViolation` represents a specific way in which the control-flow structure is incompatible with BTI enforcement:

```
enum BTIViolationKind {
    // An indirect call target does not begin with a BTI instruction.
    // This means the target is a potential ROP/JOP gadget.
    MissingBTIAtCallTarget,

    // An indirect jump target does not begin with a BTI instruction.
    MissingBTIAtJumpTarget,

    // An indirect call targets a BTI instruction of type 'j' (jump),
    // but the branch is a call (BLR), which requires type 'c' or 'jc'.
    BTITypeMismatch { expected: &'static str, found: &'static str },

    // A function with Execute in its CapD does not have a BTI
    // landing pad at its entry point. This means the function
    // cannot be safely called through a function pointer.
    ExecutableWithoutBTI,

    // A code page contains executable code but does not have
    // the PROT_BTI guardian bit set in its page flags.
    UnprotectedCodePage,
}

struct BTIViolation {
    kind: BTIViolationKind,
    source_node: NodeId,        // the indirect branch source
    target_node: Option<NodeId>, // the branch target (None if unknown)
    description: String,
}
```

### 10.4 BTI Compliance Rules

**Rule BTI-1 (Call targets → `bti c` or `bti jc`):** Every indirect call (BLR instruction) must target an instruction that begins with `bti c` or `bti jc`. If the target lacks a BTI instruction, the IVE flags a `MissingBTIAtCallTarget` violation. If the target has `bti j` (which permits jumps but not calls), the IVE flags a `BTITypeMismatch` violation with `expected = "c or jc"` and `found = "j"`.

**Rule BTI-2 (Jump targets → `bti j` or `bti jc`):** Every indirect jump (BR instruction, including return dispatch in some ABI conventions) must target an instruction that begins with `bti j` or `bti jc`. If the target lacks a BTI instruction, the IVE flags a `MissingBTIAtJumpTarget` violation.

**Rule BTI-3 (Execute → BTI):** If a function has `Execute in CapD(f)`, its entry point must begin with a BTI instruction. This is because Execute permits the function to be called through a function pointer (an indirect call), and BTI is the hardware mechanism that validates indirect call targets. A function with Execute that lacks BTI creates a window for ROP/JOP exploitation. The IVE flags an `ExecutableWithoutBTI` violation for such functions.

**Rule BTI-4 (Code page protection):** Every memory page that contains executable code must have the `PROT_BTI` guardian bit set in its page protection flags. This bit tells the hardware that BTI enforcement is required for that page. If a page contains executable code but lacks `PROT_BTI`, the IVE flags an `UnprotectedCodePage` violation.

**Rule BTI-5 (Non-executable pages):** Memory pages that do not contain any executable code must not have the execute permission bit set in their PTEs (see Section 11). This is enforced at the PTE level and is a fundamental prerequisite for BTI: if data pages are executable, an attacker can branch to any location in data without triggering a BTI fault.

---

## 11. CapD→ARM64 PTE Mapping

### 11.1 Overview

The CapD→ARM64 PTE Mapping translates VUMA's Capability Descriptor (CapD) assignments into ARM64 Page Table Entry (PTE) attributes that are enforced by the Memory Management Unit (MMU) at runtime. While Sections 6.2–6.4 describe the high-level mappings from CapD to individual hardware features, this section documents the complete, precise mapping from CapD permissions to PTE attribute fields. This mapping is implemented by the `capd_to_pte_attributes()` function, which is invoked by the VUMA code generator for each memory region to compute the PTE attributes that the runtime must install. The PTE mapping is the final step in VUMA's defense-in-depth chain: the IVE verifies the program's security properties at the software level, the code generator emits the appropriate PAC/BTI/MTE instructions, and the PTE attributes ensure that the MMU enforces the same restrictions at the hardware level.

### 11.2 `PTEAttributes` Structure

The `PTEAttributes` structure captures all ARM64 PTE fields that are relevant to VUMA's security model:

```
struct PTEAttributes {
    // Access Permissions (AP) field — controls read/write at EL0 and EL1
    // Encoding (2 bits):
    //   0b00 = EL1 read-write, EL0 no access
    //   0b01 = EL1 read-write, EL0 read-write
    //   0b10 = EL1 read-write, EL0 read-only
    //   0b11 = EL1 read-only, EL0 read-only
    ap: u8,

    // Privileged Execute-Never (PXN) — if 1, execution is never
    // permitted at EL1
    pxn: bool,

    // Unprivileged Execute-Never (UXN) — if 1, execution is never
    // permitted at EL0
    uxn: bool,

    // Execute-Never for both EL0 and EL1 (computed from PXN + UXN)
    xne: bool,  // true when both PXN and UXN are set

    // nG (non-Global) bit — if 1, TLB entries are ASID-tagged
    ng: bool,

    // AF (Access Flag) — must be set to 1 for valid PTEs
    af: bool,

    // MTE tag allocation control
    // If MTE is enabled, this field controls whether tag checks
    // are synchronous or asynchronous for this page
    mte_sync: bool,
}
```

### 11.3 `capd_to_pte_attributes()` Function

The `capd_to_pte_attributes()` function takes a CapD and the target exception level, and produces the corresponding `PTEAttributes`:

```
fn capd_to_pte_attributes(
    capd: &CapD,
    target_el: ExceptionLevel,
) -> PTEAttributes
```

The function implements the following decision logic:

1. **Determine AP from Read/Write capabilities:**
   - If `Read in CapD` and `Write in CapD`: the region is read-write.
   - If `Read in CapD` and `Write not in CapD`: the region is read-only.
   - If `Read not in CapD`: the region has no access (this should not occur in a valid VUMA program, but is handled defensively).

2. **Determine PXN/UXN from Execute capability:**
   - If `Execute in CapD`: PXN and UXN depend on the target exception level (see mapping table below).
   - If `Execute not in CapD`: both PXN=1 and UXN=1, making the page non-executable at all levels.

3. **Set nG=1 for all user-accessible pages** (VUMA always uses ASID-tagged TLB entries for isolation).

4. **Set AF=1** (VUMA pre-sets the access flag; hardware access flag faults are not used).

5. **Set mte_sync** based on the deployment MTE mode (synchronous during development, asynchronous in production).

### 11.4 Complete CapD→PTE Field Mapping Table

| CapD Configuration                     | AP    | PXN | UXN | Description                                                                 |
|----------------------------------------|-------|-----|-----|-----------------------------------------------------------------------------|
| Read-only CapD = {Read}               | 0b11  |  1  |  1  | EL0 read-only; no execution at any level                                    |
| Read-Write CapD = {Read, Write}       | 0b01  |  1  |  1  | EL0 read-write; no execution at any level                                   |
| Read-Write-DerivePtr = {Read, Write, DerivePtr} | 0b01 | 1 | 1  | EL0 read-write; PAC-signed pointers permitted; no execution                  |
| Execute CapD (EL0) = {Read, Execute}  | 0b00  |  1  |  0  | EL0 executable; PXN prevents EL1 execution (EL0 code page)                  |
| Execute CapD (EL1) = {Read, Execute}  | 0b00  |  0  |  1  | EL1 executable; UXN prevents EL0 execution (kernel code page)               |
| Execute CapD (both EL) = {Read, Execute} | 0b00 | 0  |  0  | Executable at both EL0 and EL1 (rare; only for shared trampolines)         |
| No capabilities (empty CapD)          | —     |  1  |  1  | No access; PTE marked invalid                                              |
| Send CapD = {Read, Send}              | 0b11  |  1  |  1  | Read-only like {Read}, but IVE additionally permits network serialization    |

**Key interpretation of the mapping table:**

- **Read-only CapD → AP=0b11:** This encodes read-only access at EL0 (and read-only at EL1). Combined with PXN=1 and UXN=1, the page is non-executable at all levels. This is the most restrictive data-only permission, used for constants, configuration data, and values that should never be modified or executed.

- **Read-Write CapD → AP=0b01:** This encodes read-write access at EL0. Combined with PXN=1 and UXN=1, the page is writable but non-executable. This is the default for mutable data: the program can read and write the data, but it can never be executed as code. The DerivePtr capability does not change the PTE attributes — it affects only whether PAC signing instructions are emitted (see Section 8).

- **Execute CapD → AP=0b00, PXN/UXN depends on EL:** This encodes executable access. The AP field is set to 0b00 (EL1 read-write, EL0 no access by default), but the key difference is in the PXN and UXN bits. For EL0 code pages, PXN=1 (no EL1 execution) and UXN=0 (EL0 execution permitted). For EL1 code pages, PXN=0 (EL1 execution permitted) and UXN=1 (no EL0 execution). The IVE determines the target EL from the function's security classification and the SCG region's level.

### 11.5 PTE Mapping Consistency Check

After computing PTE attributes for all memory regions, the IVE performs a consistency check: for each pair of adjacent pages, verify that no page's PTE attributes create a security hole that an adjacent page can exploit. Specifically:

1. **No writable+executable pages:** A page must never have both write permission (AP in {0b01, 0b10}) and execute permission (PXN=0 or UXN=0). This is the W^X (Write XOR Execute) policy, enforced at the PTE level. If the IVE detects a CapD that would produce such a PTE, it flags a `CrossLayerViolation`.

2. **No executable pages without BTI:** A page with execute permission must have `PROT_BTI` set (see Section 10). The IVE verifies this by checking that every page with `UXN=0 or PXN=0` has a corresponding BTI compliance result.

3. **No untagged writable pages on MTE platforms:** On platforms with MTE enabled, every writable page must have MTE tag checking enabled. The IVE verifies this by checking that `mte_sync` is set for all writable pages.

---

## 12. Graduated Security Verdict

### 12.1 Overview

The Graduated Security Verdict is the IVE's final assessment of a VUMA program's security posture. Rather than a binary pass/fail, the verdict provides a three-level classification — Secure, PartiallySecure, and Insecure — that reflects the degree to which the program's security properties are verified and enforced. This graduated approach is essential because real-world programs may have regions that are fully verified alongside regions that rely on runtime checks or hardware fallbacks. A binary verdict would either reject useful programs (false negatives) or accept programs with significant gaps (false positives). The graduated verdict enables developers to make informed decisions about whether to deploy a program, to request additional verification effort, or to accept residual risk.

### 12.2 `SecurityVerdict` Definition

The `SecurityVerdict` is a three-valued classification:

```
enum SecurityVerdict {
    // All security properties are statically verified by the IVE.
    // No runtime checks are needed. The program is secure by construction.
    Secure,

    // Most security properties are verified, but some rely on runtime
    // checks (PAC, MTE, BTI) or on assumptions that have not been
    // formally verified. The program is secure if the runtime checks
    // are not bypassed and the assumptions hold.
    PartiallySecure {
        unverified_properties: Vec<String>,
        runtime_checks: Vec<RuntimeCheckKind>,
        assumptions: Vec<Assumption>,
    },

    // One or more security properties are violated. The program
    // has known vulnerabilities and should not be deployed.
    Insecure {
        violations: Vec<SecurityViolation>,
        critical_count: usize,
        high_count: usize,
    },
}
```

### 12.3 Decision Logic

The decision logic for computing the `SecurityVerdict` is as follows:

**Step 1: Collect all violations.** Gather violations from all compliance checkers: PAC violations (Section 8), MTE violations (Section 9), BTI violations (Section 10), PTE mapping inconsistencies (Section 11), and any IVE invariant violations from Sections 1–5.

**Step 2: Classify violation severity.** Each violation is classified into one of three severity levels:

- **Critical:** A violation that enables arbitrary code execution, memory corruption, or information leakage. Examples: `MissingSignature` on a function pointer, `MissingBTIAtCallTarget` on an externally reachable function, `MissingRetag` on a deallocation of security-critical data.
- **High:** A violation that weakens a security mechanism but does not directly enable an attack. Examples: `CrossGranuleArithmetic` where the granule boundary crossing is provably safe, `BTITypeMismatch` where the mismatch is between `c` and `jc` (both permit calls).
- **Low:** A violation that is informational or that has a negligible impact on security. Examples: `UntaggedAllocation` on a read-only page, `PACBitTruncation` on a pointer that is never dereferenced.

**Step 3: Apply the decision rules.**

```
if any violation has severity == Critical:
    verdict = Insecure { violations, critical_count, high_count }

else if any violation has severity == High:
    verdict = PartiallySecure { unverified_properties, runtime_checks, assumptions }

else if all violations have severity == Low:
    verdict = PartiallySecure { unverified_properties, runtime_checks, assumptions }

else if no violations exist:
    verdict = Secure
```

**Step 4: Refine the verdict.** The initial verdict is refined based on additional context:

- **Secure → PartiallySecure downgrade:** If the program contains any region that relies on runtime checks (e.g., bounds checks inserted because the IVE could not statically prove safety), the verdict is downgraded from Secure to PartiallySecure, with the runtime checks enumerated in the `runtime_checks` field.
- **PartiallySecure → Insecure downgrade:** If the sum of high-severity and low-severity violations exceeds a configurable threshold (default: 10), the verdict is upgraded to Insecure, on the principle that a large number of minor violations indicates a systematic security problem.
- **Assumption validation:** If any assumption in the `PartiallySecure` verdict can be validated by an external tool (e.g., a model checker or a fuzzer), the validation result is recorded. If all assumptions are validated, the verdict may be upgraded to Secure.

### 12.4 Verdict Propagation

The `SecurityVerdict` is computed for the whole program, but it is also computed per-SCG-region and per-invariant. This enables fine-grained reporting:

```
struct PerRegionVerdict {
    region_id: RegionId,
    verdict: SecurityVerdict,
    invariant_verdicts: HashMap<InvariantKind, SecurityVerdict>,
}
```

Per-region verdicts allow developers to identify exactly which parts of the program need additional verification effort. For example, a program might be Secure in its cryptographic module but PartiallySecure in its networking module due to runtime checks on packet lengths. The per-invariant verdicts within each region provide even finer granularity: the networking module might be Secure for the Bounds Invariant (all packet lengths are checked) but PartiallySecure for the Information-Flow Invariant (some data flows depend on runtime checks).

### 12.5 Verdict and Deployment Policy

The deployment policy determines which verdicts are acceptable for deployment:

| Verdict          | Development | Staging | Production |
|------------------|-------------|---------|------------|
| Secure           | ✅ Allow    | ✅ Allow | ✅ Allow   |
| PartiallySecure  | ✅ Allow    | ⚠️ Allow with review | ❌ Deny (default) |
| Insecure         | ❌ Deny     | ❌ Deny  | ❌ Deny    |

The production policy for `PartiallySecure` can be overridden by a security review that examines the `unverified_properties`, `runtime_checks`, and `assumptions` fields. If the review determines that the residual risk is acceptable (e.g., the only runtime checks are bounds checks on network input, which are standard practice), the program may be allowed in production with a documented exception.

---

*End of VUMA Security Model Specification v1.1.0*
