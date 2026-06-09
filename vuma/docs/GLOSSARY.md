# VUMA Project Glossary

This document defines every term used across the VUMA project. Each entry includes the term, its pronunciation when non-obvious, a detailed definition, and cross-references to related terms. This glossary is the single source of truth for terminology; all other documents should use terms as defined here.

---

## Project Core Terms

### SCG
**Pronunciation:** /ɛs-siː-dʒiː/ (spell out: S-C-G)

**Semantic Computation Graph** — The primary representation of a program in the VUMA framework. The SCG is a directed, acyclic graph where nodes represent computational operations (function application, type construction, effect execution, resource allocation), edges represent data flow and dependency, annotations carry type information, constraints, invariants, and metadata, and regions delineate scopes, phases, security boundaries, and deployment targets. The SCG is not derived from source code — it *is* the program. Any textual or visual representation is a projection of the SCG. Two programs with the same semantics have the same SCG, regardless of how they were constructed (unique canonical form). The SCG is compositional (subgraphs combine through formally defined composition operators), transformable (semantics-preserving graph transformations are provably correct by construction), and queryable (the IVE can ask arbitrary questions about program properties).

**See also:** IVE, Projection, BD, COR

---

### IVE
**Pronunciation:** /aɪv/ (rhymes with "hive")

**Inference and Verification Engine** — The unified reasoning system that replaces the traditional compiler's type checker, borrow checker, and static analyzer. The IVE operates on the SCG and performs four core functions: (1) type inference, deriving all types from SCG structure without human annotation; (2) constraint inference, deriving temporal constraints, resource flow constraints, security boundaries, and complexity bounds from program structure; (3) verification, constructing proofs or counterexamples for properties that cannot be inferred; and (4) gradual verification, maintaining a "verification debt" of properties believed true but not yet proven, and continuously working to reduce this debt. The IVE is the reasoning heart of the VUMA framework — it is what makes verified-unsafe memory access possible, because it proves safety through global reasoning rather than local restriction.

**See also:** SCG, VUMA, MSG, Verification Debt

---

### COR
**Pronunciation:** /kɔːr/ (rhymes with "more")

**Continuous Optimization Runtime** — The runtime system that replaces the traditional compile-link-run pipeline. The COR maintains the SCG in an always-compiled state: edits trigger incremental recompilation of affected subgraphs, eliminating the concept of "build time." The COR performs profile-guided optimization by collecting runtime profile data and feeding it back to the IVE for optimization decisions. It supports speculative optimization (pre-optimizing likely execution paths with transparent fallback) and adaptive deployment (moving computation between nodes in a distributed system based on latency, cost, and availability). The COR treats the SCG's region annotations as guidance for deployment topology decisions.

**See also:** SCG, IVE, Projection

---

### BD
**Pronunciation:** /biː-diː/ (spell out: B-D)

**Behavioral Descriptor** — A triple `(RepD, CapD, RelD)` that describes data by what it *does* rather than what it *is called*. The BD replaces traditional nominal types in the VUMA framework. Where a traditional type bundles memory layout, valid operations, and relationships into a single nominal category, the BD decomposes these into three orthogonal dimensions that can each vary independently. The IVE infers and verifies all three components of a BD from program structure, function signatures, and execution context. The programmer can add explicit descriptors as optional refinements, but they are never mandatory annotations. Two values with the same BD are interchangeable regardless of their nominal "type name," and two values with different BDs are distinct even if they share a name.

**See also:** RepD, CapD, RelD, IVE

---

### RepD
**Pronunciation:** /rɛp-diː/ (rhymes with "step-dee")

**Representation Descriptor** — The component of a Behavioral Descriptor that specifies the physical layout of data in memory: size, alignment, field offsets, and bit-level structure. A RepD is not a type; it is a *memory map*. Multiple RepDs can describe the same memory at different granularities simultaneously — a 128-byte region can be described as `bytes[128]`, as `float32[32]`, or as `struct { header: uint32; payload: bytes[124] }`. The RepD does not choose among these; it enumerates all valid interpretations. This multi-view capability is what enables zero-copy interoperation and eliminates type-conversion bugs, because there is no conversion — only a shift in perspective verified by the IVE.

**See also:** BD, CapD, RelD, VUMA

---

### CapD
**Pronunciation:** /kæp-diː/ (rhymes with "map-dee")

**Capability Descriptor** — The component of a Behavioral Descriptor that specifies what operations are valid on data in a given context. A CapD enumerates permissions: read, write, iterate, send over network, persist to disk, execute as code, derive pointer from, compare for equality, hash, serialize. A CapD is a set of *permissions*, not a type class. The same data can have different capabilities in different contexts — a buffer is readable and writable during processing but only readable during transmission. CapD captures this context-dependence natively, enabling polymorphism through capability matching: any value with the required capabilities satisfies the constraint, regardless of its nominal type.

**See also:** BD, RepD, RelD, Capability Calculi

---

### RelD
**Pronunciation:** /rɛl-diː/ (rhymes with "tell-dee")

**Relational Descriptor** — The component of a Behavioral Descriptor that specifies relationships between data values: temporal co-occurrence, structural containment, dependency ordering, semantic equivalence, and security-level flow. A RelD captures the web of relationships that nominal types express through inheritance hierarchies and trait implementations, but with far greater expressiveness. For example, a RelD can express: "this value is semantically equivalent to that value but represented differently" (a database row and a protobuf message describing the same entity), or "this value must not outlive that value" (a slice and its backing buffer), or "this value's security level is derived from the maximum security level of its sources" (a computed result combining public and secret inputs).

**See also:** BD, RepD, CapD

---

### VUMA
**Pronunciation:** /vuːmɑː/ (rhymes with "llama" with a v)

**Verified-Unsafe Memory Access** — The memory model at the core of this project, which permits unrestricted raw memory access (pointers, addresses, manual allocation, arbitrary casts) and establishes safety through global verification rather than local restriction. In the VUMA model, all data access is pointer-based. There are no "safe references" and "unsafe pointers" — there are only addresses, and the IVE verifies that every access through every address is valid at the point of access. The memory model has three primitives: Address (a numeric value identifying a location), Region (a contiguous range of addresses with allocation status, ownership context, and access history), and Access (a read or write operation targeting an address, verified by the IVE before execution). VUMA inverts the safety model: instead of "restrict by default, permit with explicit unsafe blocks," it proposes "permit by default, verify by global reasoning, flag only what cannot be proven safe."

**See also:** IVE, MSG, Liveness, Exclusivity, Interpretation, Origin, Cleanup

---

### MSG
**Pronunciation:** /ɛm-ɛs-dʒiː/ (spell out: M-S-G)

**Memory State Graph** — The IVE's formal model of the program's entire memory behavior. The MSG captures every allocation point and the region it creates, every pointer derivation and the path from allocation to dereference, every deallocation point and the region it destroys, every concurrent access and the synchronization that orders it, and every cast or reinterpretation and the representation descriptors involved. The IVE proves global memory invariants (liveness, exclusivity, interpretation, origin, cleanup) against the MSG. The MSG is the data structure that makes VUMA possible — it is the comprehensive model against which all memory safety proofs are constructed.

**See also:** VUMA, IVE, Liveness, Exclusivity, Interpretation, Origin, Cleanup

---

### Projection
**Pronunciation:** /prəˈdʒɛkʃən/

A view of the SCG rendered for human consumption. Projections come in four forms: (1) Textual projections — traditional code-like views customized to the viewer's role (systems programmer, domain expert, security auditor); (2) Visual projections — dataflow diagrams, call graphs, state machines, memory layout views that are live and interactive; (3) Conversational projections — natural language dialogue where the human describes intent and the system translates it into SCG modifications; and (4) Diff projections — change descriptions in human terms ("the authentication flow now requires 2FA for admin accounts") rather than line-level diffs. Projections are bidirectional: changes to the projection are propagated back to the SCG and validated by the IVE before being applied.

**See also:** SCG, IVE

---

### Outcome Space
**Pronunciation:** /ˈaʊtkʌm speɪs/

The complete set of possible outcomes of a computation, including all failure modes. In the VUMA framework, the type of a function includes its complete outcome space — not just `Result<T, E>` with a generic error type, but a structured enumeration of every possible outcome with its conditions and handlers. The IVE verifies that every outcome is handled, either explicitly by the programmer or implicitly by a verified safe default. As the program executes and invariants are established, the outcome space shrinks — for example, after authentication succeeds, the "unauthorized" outcome is removed from the possibility space for subsequent operations.

**See also:** IVE, BD, CapD

---

### Verification Debt
**Pronunciation:** /ˌvɛrɪfɪˈkeɪʃən dɛt/

Properties believed true but not yet formally proven by the IVE. The IVE maintains verification debt as a prioritized work list: it continuously works to reduce debt, prioritizing properties that affect correctness and security. The concept mirrors technical debt in software engineering, but applies to formal guarantees rather than code quality. Verification debt is acceptable in practice because not all properties need to be proven at all times — but the system tracks and communicates the current debt level so that stakeholders can make informed decisions about risk.

**See also:** IVE, Verification Confidence

---

### Verification Confidence
**Pronunciation:** /ˌvɛrɪfɪˈkeɪʃən ˈkɒnfɪdəns/

A tiered assessment of proof strength assigned to each verified property: (1) **Proven safe** — the IVE has constructed a formal proof; (2) **Probably safe given stated assumptions** — the IVE has proven safety conditional on assumptions that it cannot verify but are documented; (3) **Unverified** — the IVE has not yet been able to establish safety. Deployment policies can require minimum confidence levels for different environments, allowing the system to gracefully degrade when full verification is not achievable.

**See also:** IVE, Verification Debt, VUMA

---

## Verification Invariant Terms

### Liveness
**Pronunciation:** /ˈlaɪvnəs/

The VUMA invariant requiring that every memory access targets a region that is allocated at that program point. The IVE proves liveness by tracking allocation and deallocation events through the MSG and verifying that no execution path leads to a dereference of a freed or unallocated region. If the IVE cannot prove liveness for a specific access, it flags that access and provides the execution path that leads to the potential use-after-free. Liveness is the first of the five VUMA global invariants and corresponds to the elimination of use-after-free bugs.

**See also:** VUMA, MSG, Exclusivity, Origin, Cleanup

---

### Exclusivity
**Pronunciation:** /ˌɛkskluːˈsɪvɪti/

The VUMA invariant requiring that every write access does not overlap with a simultaneous read or write access through a different address. The IVE proves exclusivity by analyzing concurrent access patterns through the MSG, verifying that either accesses are properly ordered by synchronization or that they target non-overlapping regions. If the IVE cannot prove exclusivity, it flags the potential data race and provides the concurrent execution paths. Exclusivity is the second VUMA invariant and corresponds to the elimination of data races.

**See also:** VUMA, MSG, Liveness, Interpretation

---

### Interpretation
**Pronunciation:** /ɪnˌtɜːprɪˈteɪʃən/

The VUMA invariant requiring that every memory access interprets the target bytes according to a valid representation descriptor. If the IVE cannot prove the interpretation is valid (for example, reading uninitialized memory as a pointer, or interpreting a floating-point bit pattern as an integer when the subsequent operations require an integer), it flags the access. Interpretation is the third VUMA invariant and ensures that the bytes at any accessed address are meaningful under the operation being performed, eliminating type confusion and uninitialized-memory bugs.

**See also:** VUMA, MSG, RepD, Liveness

---

### Origin
**Pronunciation:** /ˈɒrɪdʒɪn/

The VUMA invariant requiring that every address can be traced back to a valid allocation point. If an address is computed through arithmetic that the IVE cannot trace to an allocation (for example, a hardcoded constant like `0xDEADBEEF`, or a value read from an untrusted source), the IVE flags the computation. Origin is the fourth VUMA invariant and prevents the creation of "phantom pointers" that point to memory the program never allocated, which could corrupt arbitrary process memory.

**See also:** VUMA, MSG, Liveness, Cleanup

---

### Cleanup
**Pronunciation:** /ˈkliːnʌp/

The VUMA invariant requiring that every allocated region is eventually freed, or explicitly marked as intentionally leaked (for example, a long-lived arena or a global static). If the IVE detects a potential leak, it flags the allocation. Cleanup is the fifth VUMA invariant and corresponds to the elimination of memory leaks. The "explicitly marked as intentionally leaked" exception accommodates patterns where memory is intentionally never freed (program-lifetime arenas, static globals), distinguishing them from accidental leaks.

**See also:** VUMA, MSG, Liveness, Origin

---

## ARM64 / AArch64 Terms

### AAPCS64
**Pronunciation:** /æpks-sɪkstiːfɔːr/ (spell out acronym)

**ARM Architecture Procedure Call Standard for 64-bit** — The ABI (Application Binary Interface) that governs how functions are called on ARM64 processors. AAPCS64 defines which registers are used for arguments (x0–x7 for integer/pointer arguments, v0–v7 for floating-point/SIMD arguments), which registers are caller-saved vs. callee-saved, stack frame layout, alignment requirements (16-byte stack alignment), and return value conventions. VUMA codegen must comply with AAPCS64 to interoperate with C libraries and the operating system on Pi 5.

**See also:** Cortex-A76, BCM2712

---

### DMB
**Pronunciation:** /diː-ɛm-biː/ (spell out)

**Data Memory Barrier** — An ARM64 instruction that ensures that all explicit memory accesses before the DMB complete before any explicit memory accesses after the DMB are observed. DMB is used to enforce ordering between memory operations in multi-processor or DMA scenarios. In VUMA, the IVE must understand DMB semantics to correctly verify the exclusivity invariant across concurrent access patterns. DMB does not cause a pipeline flush; it only orders memory operations as observed by other observers.

**See also:** DSB, ISB, Exclusivity

---

### DSB
**Pronunciation:** /diː-ɛs-biː/ (spell out)

**Data Synchronization Barrier** — An ARM64 instruction stronger than DMB: it ensures that all explicit memory accesses before the DSB complete before any instruction after the DSB is executed. DSB is required when the CPU must wait for memory operations to be truly complete (for example, after writing to a device register before reading a status register). In VUMA, DSB is used in device-driver code for Pi 5 peripherals and in memory-mapped I/O scenarios where the IVE must model device-visible memory ordering.

**See also:** DMB, ISB, BCM2712

---

### ISB
**Pronunciation:** /aɪ-ɛs-biː/ (spell out)

**Instruction Synchronization Barrier** — An ARM64 instruction that flushes the processor pipeline so that all instructions after the ISB are fetched from the instruction cache or memory after the ISB completes. ISB is required after writing to system registers (such as the SCTLR, TLB entries, or branch predictor settings) to ensure the change takes effect before subsequent instructions execute. In VUMA, ISB is relevant for context-switching code and low-level system programming on Pi 5.

**See also:** DMB, DSB

---

### LDXR
**Pronunciation:** /ɛl-diː-ɛks-ɑːr/ (spell out)

**Load Exclusive Register** — An ARM64 instruction that loads a value from memory and tags the memory address for exclusive access monitoring. LDXR is one half of the ARM64 exclusive access pair (with STXR) that implements lock-free atomic operations. The hardware tracks whether the tagged address has been written to by another agent between the LDXR and the subsequent STXR. In VUMA, LDXR/STXR pairs are the codegen target for atomic compare-and-swap operations, and the IVE must understand their semantics to verify the exclusivity invariant for lock-free data structures.

**See also:** STXR, Exclusivity

---

### STXR
**Pronunciation:** /ɛs-tiː-ɛks-ɑːr/ (spell out)

**Store Exclusive Register** — An ARM64 instruction that attempts to store a value to memory, succeeding only if the target address has not been written to by another agent since the corresponding LDXR. STXR returns a status value in a register: 0 indicates success, 1 indicates failure (the exclusive monitor was lost). In VUMA, STXR is the codegen target for the store side of atomic compare-and-swap. Failed STXR operations typically cause a retry loop, and the IVE must verify that such loops eventually succeed (a liveness property).

**See also:** LDXR, Liveness, Exclusivity

---

### Cortex-A76
**Pronunciation:** /ˈkɔːtɛks eɪ sɛvənti-sɪks/

The ARM-compatible 64-bit CPU core used in the Raspberry Pi 5's BCM2712 SoC. The Cortex-A76 is an out-of-order, superscalar processor implementing the ARMv8.2-A architecture. It features a 4-wide decode pipeline, branch prediction, and a deep reorder buffer that allows significant instruction-level parallelism. For VUMA, the Cortex-A76's memory model (weakly ordered with multi-copy atomicity) is the target for the IVE's exclusivity verification, and its instruction set is the target for the ARM64 codegen backend.

**See also:** BCM2712, AAPCS64

---

## Raspberry Pi 5 Terms

### BCM2712
**Pronunciation:** /biː-siː-ɛm tuː-θɜːtiːn/ (spell out)

The system-on-chip (SoC) at the heart of the Raspberry Pi 5. The BCM2712 contains a quad-core ARM Cortex-A76 processor clocked at 2.4 GHz, a VideoCore VII GPU, and a comprehensive set of peripherals including PCIe 2.0, USB 3.0, Gigabit Ethernet, and the Pi 5's extended GPIO. The BCM2712 is the primary hardware target for VUMA codegen and verification: all ARM64 assembly output must execute correctly on this SoC, and the IVE must model its memory model, cache hierarchy, and peripheral address map.

**See also:** Cortex-A76, GPIO, UART

---

### GPIO
**Pronunciation:** /dʒiː-piː-aɪ-oʊ/ (spell out)

**General-Purpose Input/Output** — The configurable digital pins on the Raspberry Pi 5 that can be used for a wide variety of hardware interfacing tasks. The Pi 5 exposes a 40-pin GPIO header, with pins supporting digital I/O, PWM, I2C, SPI, and UART protocols. In VUMA, GPIO access is performed through memory-mapped I/O: the BCM2712 maps GPIO registers into the physical address space (starting at 0x7E200000 for the legacy view or the corresponding mapped address in the ARM physical map). The IVE must model these as device memory with specific ordering requirements.

**See also:** BCM2712, UART, DMB

---

### UART
**Pronunciation:** /juː-ɑːrt/ (rhymes with "part")

**Universal Asynchronous Receiver-Transmitter** — The serial communication peripheral used on the Raspberry Pi 5 for console I/O and debug output. The BCM2712 includes a PL011 UART (the primary UART for Bluetooth on the Pi 5) and a mini UART (used for the serial console). In VUMA, UART output is the first I/O capability implemented, providing a way to print verification results and program output before more complex drivers (USB, Ethernet) are available. UART registers are memory-mapped and accessed with device memory ordering semantics.

**See also:** BCM2712, GPIO

---

## Type Theory Terms

### Nominal Types
**Pronunciation:** /ˈnɒmɪnəl taɪps/

Types defined by their *name* and explicit declaration. Two nominal types are considered distinct even if they have identical structure — for example, `struct UserId(u64)` and `struct OrderId(u64)` are different types in Rust despite having the same memory layout, because they have different names. Nominal typing is the dominant paradigm in mainstream languages (C, Java, Rust, Swift) because it aligns with how humans categorize the world. In VUMA, nominal types are superseded by Behavioral Descriptors, which define data by what it *does* (representation, capabilities, relationships) rather than by what it is *called*.

**See also:** Structural Types, Behavioral Types, BD

---

### Structural Types
**Pronunciation:** /ˈstrʌktʃərəl taɪps/

Types defined by their *structure* rather than their name. Two structural types are considered equivalent if they have the same shape, regardless of what they are called. TypeScript interfaces, Go structural typing for interfaces, and ML record types are examples. Structural typing is more flexible than nominal typing for interoperation (any value with the right shape satisfies the interface), but it can accidentally equate types that happen to share structure but have different semantic intent. VUMA's Behavioral Descriptors generalize structural types: instead of comparing shape, they compare capabilities and relationships, capturing the useful properties of structural typing while avoiding accidental equivalence.

**See also:** Nominal Types, Behavioral Types, CapD, RelD

---

### Behavioral Types
**Pronunciation:** /bɪˈheɪvjərəl taɪps/

A type formalism that describes the *behavior* of a component — the sequence and conditions of interactions it may engage in — rather than just its data layout. Behavioral types originate in concurrency theory (session types, behavioral contracts) and specify communication protocols, ordering constraints, and resource usage patterns. VUMA's Behavioral Descriptors are a form of behavioral type that extends the concept from communication protocols to all properties of data: what operations are valid (CapD), what memory layout is used (RepD), and what relationships exist with other data (RelD). This makes VUMA's type system a full behavioral type system, not just a data classification system.

**See also:** Nominal Types, Structural Types, BD, CapD

---

### Capability Calculi
**Pronunciation:** /ˌkeɪpəˈbɪlɪti ˈkælkjʊlaɪ/

A family of formal calculi that model capabilities as first-class, unforgeable tokens that grant specific access rights. Capability calculi (such as the object-capability model, capability-based security, and the calculus of capabilities) provide formal foundations for reasoning about which components can access which resources. VUMA's CapD system draws on capability calculi theory: a CapD is essentially a capability set, and the IVE verifies that no operation exceeds the capabilities granted to its inputs. The key difference from traditional capability systems is that VUMA capabilities are *inferred* by the IVE rather than explicitly managed by the programmer.

**See also:** CapD, BD, IVE

---

## Additional Project Terms

### Derivation Chain
**Pronunciation:** /dɛrɪˈveɪʃən tʃeɪn/

The sequence of operations by which a pointer is computed from an original allocation. For example, if `base = allocate(bytes[1024])`, `offset = base + 64`, and `field_ptr = offset as *Header`, then the derivation chain for `field_ptr` is `[base, offset, field_ptr]`. The IVE tracks derivation chains through the MSG to verify the origin invariant (every address traces back to an allocation) and to verify that derived pointers remain within the bounds of their originating region.

**See also:** MSG, Origin, VUMA

---

### Region (SCG)
**Pronunciation:** /ˈriːdʒən/

A subgraph of the SCG that delineates a scope, phase, security boundary, or deployment target. SCG regions are the unit of composition, optimization, and deployment. The COR uses region annotations to guide adaptive deployment decisions (moving computation between nodes based on latency and cost constraints). In the VUMA memory model, a Region is also a contiguous range of addresses with associated metadata (allocation status, ownership context, access history). The dual usage is intentional: SCG regions correspond to ownership contexts, and memory regions are the physical manifestation of those contexts.

**See also:** SCG, COR, MSG

---

### Verification Debt
**Pronunciation:** /ˌvɛrɪfɪˈkeɪʃən dɛt/

(Defined above; listed here for alphabetical completeness.)

**See also:** IVE, Verification Confidence

---

### VUMA-VERIFIED
**Pronunciation:** /ˈvuːmɑː ˈvɛrɪfaɪd/

A code annotation comment (`// VUMA-VERIFIED`) used in the VUMA Rust codebase to mark methods and functions whose safety has been formally verified by the IVE. This comment replaces the `unsafe` keyword in semantic intent: where Rust would require an `unsafe` block to acknowledge that the programmer takes responsibility for safety, `// VUMA-VERIFIED` indicates that the IVE has taken that responsibility and proven safety. No VUMA stdlib code should contain bare `unsafe` blocks — all raw memory access should be either `// VUMA-VERIFIED` or `// IVE-TODO`.

**See also:** IVE-TODO, VUMA, Conventions

---

### IVE-TODO
**Pronunciation:** /aɪv tuːˈduː/

A code annotation comment (`// IVE-TODO`) used in the VUMA Rust codebase to mark methods and functions where IVE verification has not yet been implemented. This is the equivalent of Rust's `unsafe` block in terms of acknowledging that safety has not been formally proven, but it carries a different connotation: it is a *temporary* state, not an accepted risk. The project tracks IVE-TODO items as verification debt that must be resolved before stable releases.

**See also:** VUMA-VERIFIED, Verification Debt, IVE

---

*This glossary is maintained as part of the VUMA project documentation. To add a term, submit a PR with the term added in alphabetical order, following the format above. Every term must have a definition of at least 50 words and at least one "See also" cross-reference.*
