# Contributing to VUMA

Thank you for contributing to the VUMA project — the Verified-Unsafe Memory Access
framework for AI-native programming language design. This guide explains how to set
up your development environment, make changes, and get them merged.

---

## Table of Contents

1. [How to Build the Project](#1-how-to-build-the-project)
2. [How to Run Tests](#2-how-to-run-tests)
3. [Test Infrastructure](#3-test-infrastructure)
4. [How to Add New SCG Node Types](#4-how-to-add-new-scg-node-types)
5. [How to Add New Verification Passes](#5-how-to-add-new-verification-passes)
6. [How to Add New Backend Instructions](#6-how-to-add-new-backend-instructions)
7. [Code Review Process](#7-code-review-process)
8. [PR Template](#8-pr-template)

---

## 1. How to Build the Project

### 1.1 Prerequisites

| Tool | Minimum Version | Purpose |
|------|----------------|---------|
| Rust toolchain (stable) | Per `rust-toolchain.toml` | Core language; VUMA uses the stable channel |
| `rustup` | Latest | Rust toolchain management |
| `cargo` | Included with Rust | Build system |
| Git | 2.30+ | Version control |
| QEMU (aarch64) | 7.0+ | ARM64 emulation for testing |
| `aarch64-none-elf-` toolchain | Latest | Bare-metal cross-compilation (objcopy, ld) |
| ARM64 hardware | Any | Physical target (optional but recommended) |

### 1.2 Install the Rust Toolchain

The project uses `rust-toolchain.toml` to pin the toolchain. When you enter the
repository, `rustup` will automatically select the correct channel and components.

```bash
# Install rustup if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# The targets are listed in rust-toolchain.toml and will be auto-installed:
#   stable, components: rustfmt, clippy
#   targets: aarch64-unknown-linux-gnu, aarch64-unknown-none

# Verify the toolchain
rustup show
cargo --version
```

### 1.3 Clone and Build

```bash
git clone https://github.com/vuma-project/vuma.git
cd vuma

# Build all workspace members (12 crates)
cargo build --workspace

# Quick check without generating binaries (faster iteration)
cargo check --workspace
```

The workspace consists of the following crates:

| Crate | Path | Purpose |
|-------|------|---------|
| `vuma-scg` | `src/scg/` | Semantic Computation Graph data structures and algorithms |
| `vuma-ive` | `src/ive/` | Inference and Verification Engine |
| `vuma` (core) | `src/vuma/` | VUMA memory model: Address, Region, Access, MSG |
| `vuma-bd` | `src/bd/` | Behavioral Descriptors (RepD, CapD, RelD) |
| `vuma-cor` | `src/cor/` | Continuous Optimization Runtime |
| `vuma-projection` | `src/projection/` | Projection system (textual, visual, conversational) |
| `vuma-parser` | `src/parser/` | SCG parser and projection parser |
| `vuma-codegen` | `src/codegen/` | Multi-architecture code generation (10 backends) |
| `vuma-std` | `src/std/` | VUMA standard library |
| `vuma-proof` | `src/proof/` | Proof infrastructure and formal methods |
| `vuma-tests` | `src/tests/` | Integration tests |

### 1.4 Build with Make or Just

The project includes both a `Makefile` and a `justfile` for common tasks:

```bash
# Using make
make build          # cargo build --workspace
make test           # cargo test --workspace
make check          # cargo check --workspace
make doc            # cargo doc --workspace --no-deps
make fmt            # cargo fmt --all
make clippy         # cargo clippy --workspace -- -D warnings
make clean          # cargo clean

# Using just
just build
just test
just clean
```

### 1.5 Cross-Compilation

#### Userspace (Linux ARM64)

If you are developing on an x86_64 host and targeting ARM64 with Linux:

```bash
# The target is already configured in rust-toolchain.toml
# Install the ARM64 Linux linker
# On Ubuntu/Debian:
sudo apt install gcc-aarch64-linux-gnu

# Build for ARM64 Linux
cargo build --target aarch64-unknown-linux-gnu
```

#### Bare-Metal (aarch64-unknown-none)

For bare-metal ARM64 targets (no OS), the project provides a complete build pipeline:

```bash
# Build the bare-metal kernel
make bare-metal

# Build kernel8.img from the ELF binary (requires aarch64-none-elf-objcopy)
make bare-metal-image

# Flash kernel8.img to an SD card boot partition
make bare-metal-flash SD=/mnt/sd-boot

# Launch a QEMU debug session with GDB stub on :1234
make bare-metal-debug
```

The bare-metal build uses a linker script and build script. The linker script defines
the memory layout (RAM at `0x80000`, 8 MiB; MMIO window at `0x100000`, 1 MiB),
section ordering (`.text.boot` → `.text` → `.rodata` → `.data` → `.bss`),
and per-core stacks (64 KiB × 4 cores).

### 1.6 QEMU Setup for ARM64 Testing

```bash
# Install QEMU with ARM64 support
# On Ubuntu/Debian:
sudo apt install qemu-system-arm

# Run a test binary under QEMU (userspace)
qemu-aarch64 -L /usr/aarch64-linux-gnu target/aarch64-unknown-linux-gnu/debug/vuma-test

# Run the bare-metal kernel in QEMU (uses raspi3b as closest model)
make bare-metal-debug
```

### 1.7 Formatting and Linting

```bash
# Check formatting (CI enforces this)
cargo fmt --check

# Apply formatting
cargo fmt --all

# Run clippy with warnings-as-errors (CI enforces this)
cargo clippy --workspace -- -D warnings
```

The project uses `rustfmt.toml` (max_width = 100, tab_spaces = 4, edition = "2021")
and `clippy.toml` (cognitive-complexity-threshold = 50).

---

## 2. How to Run Tests

### 2.1 Unit Tests

Run all unit tests across the workspace:

```bash
cargo test --workspace
```

Run tests for a specific crate:

```bash
cargo test -p vuma-ive
cargo test -p vuma-scg
cargo test -p vuma-codegen
cargo test -p vuma-bd
cargo test -p vuma-vuma
cargo test -p vuma-proof
cargo test -p vuma-cor
```

Run a specific test by name:

```bash
cargo test -p vuma-ive -- verify_liveness_catches_use_after_free
cargo test -p vuma-scg -- integration_test_build_validate_query
cargo test -p vuma-bd -- test_repd_compat
```

### 2.2 Integration Tests

Integration tests live in `src/tests/` and exercise cross-crate workflows:

```bash
cargo test -p vuma-tests
```

The integration test crate includes tests for BD inference, trivial proofs,
concurrent access patterns, doubly-linked list verification, and graph
construction workflows.

### 2.3 Verification Tests

The IVE has its own verification test suite that exercises the full
verification pipeline across all five invariants:

```bash
# Run IVE verification tests (may be slow; serial execution avoids races)
cargo test -p vuma-ive -- --test-threads=1 -- verification

# Run with verbose output to see verification details
cargo test -p vuma-ive -- --nocapture -- verification

# Run a specific invariant's tests
cargo test -p vuma-ive -- liveness
cargo test -p vuma-ive -- exclusivity
cargo test -p vuma-ive -- interpretation
cargo test -p vuma-ive -- origin
cargo test -p vuma-ive -- cleanup
```

### 2.4 Codegen Tests

ARM64 codegen tests produce assembly output that is compared against
expected outputs. These tests verify instruction encoding, register
allocation, and the SCG → IR → ARM64 pipeline:

```bash
cargo test -p vuma-codegen

# The codegen crate tests instruction encoding against the ARM Architecture
# Reference Manual. Each Instruction variant has an encode() method that
# produces a 32-bit machine code word.
```

### 2.5 Proof Tests

The proof infrastructure tests verify formal proof construction,
checking, counterexample generation, and proof tactics:

```bash
cargo test -p vuma-proof
```

### 2.6 Benchmarks

```bash
# Run benchmarks (requires nightly or stable with test feature)
cargo bench --workspace

# Or via Make
make bench
```

### 2.7 Full CI Check

Before submitting a PR, run the full CI check locally:

```bash
cargo fmt --check &&
cargo clippy --workspace -- -D warnings &&
cargo test --workspace &&
cargo test -p vuma-ive -- --test-threads=1 -- verification
```

---

## 3. Test Infrastructure

The VUMA project has a comprehensive test infrastructure spanning multiple categories:

### 3.1 Test Categories

| Category | Crate | What It Tests |
|----------|-------|---------------|
| Cross-backend consistency | `vuma-tests::cross_backend` | All 10 backends produce consistent results |
| ABI conformance | `vuma-tests::abi_conformance` | 27 ABI compliance tests across backends |
| ELF validation | `vuma-tests::elf_validation` | 7 native backends produce valid ELF binaries |
| Wasm validation | `vuma-tests::wasm_validation` | 12 Wasm32 module validation tests |
| Property-based testing | `vuma-tests::property_tests` | Proptest-based fuzzing of compiler components |
| Parser roundtrip | `vuma-tests::parser_roundtrip` | Parse → SCG → text → re-parse stability |
| SHA256d backends | `vuma-tests::sha256d_backends` | SHA256d correctness across all backends |
| DWARF/FFI integration | `vuma-tests::dwarf_ffi_integration` | Debug info and FFI syscall emission |
| Diagnostics integration | `vuma-tests::diagnostics_integration` | 65 diagnostic codes and error chaining |
| Memory safety | `vuma-tests::execution_validation` | 10 violation types, runtime bounds checks |
| Regression tests | `vuma-tests::regression` | Prevents re-introduction of fixed bugs |

### 3.2 Running Specific Test Categories

```bash
# Cross-backend consistency (10 backends)
cargo test -p vuma-tests -- cross_backend

# ABI conformance (27 tests)
cargo test -p vuma-tests -- abi_conformance

# ELF validation (7 native backends)
cargo test -p vuma-tests -- elf_validation

# Wasm validation (12 tests)
cargo test -p vuma-tests -- wasm_validation

# Property-based testing
cargo test -p vuma-tests -- property

# DWARF and FFI integration
cargo test -p vuma-tests -- dwarf_ffi

# Diagnostics system
cargo test -p vuma-tests -- diagnostics
```

### 3.3 Benchmarks

The benchmark suite covers SHA256d across all 10 backends, compilation speed, binary size comparison, and codegen quality (redundant load/store analysis):

```bash
make bench
# or: cargo bench --workspace
```

### 3.4 CI Matrix

The GitHub Actions CI runs on every push and PR:
- Format check (`cargo fmt --check`)
- Lint check (`cargo clippy -- -D warnings`)
- Full test suite across all crates
- Cross-compile matrix for all 10 targets
- Dependabot for dependency updates

---

## 4. How to Add New SCG Node Types

The Semantic Computation Graph is extensible. Adding a new node type involves
coordinated changes across the `vuma-scg`, `vuma-ive`, `vuma-codegen`, and
`vuma-tests` crates.

### 3.1 Step 1: Define the Node Payload (vuma-scg)

Add a new struct in `src/scg/src/node.rs` representing the node's type-specific
data. Follow the existing pattern used by `AllocationNode`, `AccessNode`,
`ComputationNode`, etc.:

```rust
/// Data specific to a barrier synchronization node.
///
/// Represents a memory barrier operation that enforces ordering
/// between preceding and subsequent memory accesses. Maps to
/// ARM64 DMB/DSB/ISB instructions depending on the barrier kind.
///
/// # SCG Semantics
///
/// - Input edges: preceding access nodes
/// - Output edges: subsequent access nodes
/// - Effects: memory ordering constraint
/// - Invariants: exclusivity verification must respect barrier ordering
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarrierNode {
    /// The kind of memory barrier (DataMemory, DataSync, InstructionSync).
    pub barrier_kind: BarrierKind,
    /// The shareability domain of the barrier (InnerShareable, OuterShareable, FullSystem).
    pub domain: BarrierDomain,
}
```

### 3.2 Step 2: Register the Node Type Variant

Add the variant to the `NodeType` enum in `src/scg/src/node.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeType {
    Computation,
    Allocation,
    Deallocation,
    Access,
    Cast,
    Effect,
    Control,
    Phantom,
    Barrier,  // New variant
}
```

Update the `Display` impl for `NodeType`:

```rust
NodeType::Barrier => write!(f, "Barrier"),
```

### 3.3 Step 3: Add the Node Payload Variant

Add the variant to `NodePayload` and the corresponding match arms:

```rust
pub enum NodePayload {
    // ... existing variants ...
    /// Payload for `NodeType::Barrier`.
    Barrier(BarrierNode),
}
```

### 3.4 Step 4: Re-export from the Crate Root

In `src/scg/src/lib.rs`, add the new types to the re-exports:

```rust
pub use node::{
    // ... existing exports ...
    BarrierNode, BarrierKind, BarrierDomain,
};
```

### 3.5 Step 5: Update Graph Construction

Update `src/scg/src/graph.rs` to handle the new node type in any
validation or traversal logic. The `SCG::add_node()` method accepts
`NodeType` + `NodePayload`, so ensure validation covers the new variant.

### 3.6 Step 6: Add IVE Verification (vuma-ive)

Add a verification function for the new node in `src/ive/src/`. The IVE
has per-invariant modules (`liveness.rs`, `exclusivity.rs`, `interpretation.rs`,
`origin.rs`, `cleanup.rs`) — update whichever invariants are affected by the
new node type. For a barrier node, the exclusivity pass must respect barrier
ordering:

```rust
// In src/ive/src/exclusivity.rs or a new module

/// Verify that barrier nodes correctly order memory accesses for
/// exclusivity verification.
///
/// Checks:
/// - The barrier node has at least one predecessor access node
/// - The barrier node has at least one successor access node
/// - The barrier's domain covers all concurrent access pairs it separates
pub fn verify_barrier_ordering(
    node: &BarrierNode,
    msg: &MSG,
) -> Result<VerificationResult, IveError> {
    // Implementation here
}
```

### 3.7 Step 7: Add Codegen (vuma-codegen)

Add the instruction selection and lowering rule in `src/codegen/src/arm64.rs`
and `src/codegen/src/scg_to_ir.rs`:

```rust
// In the instruction selector in src/codegen/src/arm64.rs
// Map the Barrier SCG node to the appropriate ARM64 instruction:
//   BarrierKind::DataMemory + BarrierDomain::InnerShareable → DMB ISH
//   BarrierKind::DataSync + BarrierDomain::FullSystem → DSB SY
//   BarrierKind::InstructionSync → ISB
```

### 3.8 Step 8: Add Tests

Add tests at every level:

1. **Unit tests** in `src/scg/src/node.rs` — test construction and display of the
   new `BarrierNode` and `NodeType::Barrier`
2. **Graph tests** in `src/scg/src/graph.rs` — test that an SCG containing barrier
   nodes validates correctly
3. **IVE tests** in `src/ive/src/` — test that the verification pass correctly
   accepts and rejects barrier configurations
4. **Codegen tests** in `src/codegen/` — test that barrier nodes lower to the
   correct ARM64 instruction encoding
5. **Integration test** in `src/tests/` — exercise the full pipeline:
   SCG construction → IVE verification → codegen → assembly output check

---

## 5. How to Add New Verification Passes

VUMA currently defines five global invariants (liveness, exclusivity,
interpretation, origin, cleanup). Adding a new verification pass requires
careful design and cross-crate coordination.

### 4.1 Step 1: Define the Invariant

Add the invariant to the `InvariantKind` enum in
`src/ive/src/invariant_aggregator.rs`:

```rust
pub enum InvariantKind {
    Liveness,
    Exclusivity,
    Interpretation,
    Origin,
    Cleanup,
    Alignment,  // New: every access is properly aligned for its RepD
}
```

### 4.2 Step 2: Write the Formal Specification

Create a formal specification document in `docs/spec/` describing the invariant.
The specification must include:

- **Definition**: What property does the invariant guarantee?
- **Scope**: Which SCG nodes and MSG edges does it apply to?
- **Verification algorithm**: How does the IVE check this invariant?
  What is the time complexity?
- **Counterexample format**: When verification fails, what information is reported?
- **Relationship to existing invariants**: Is the new invariant independent of,
  implied by, or does it imply any existing invariant?
- **Interaction with ARM64 memory model**: How does the weakly-ordered
  memory model affect this invariant?

### 4.3 Step 3: Implement the Verification Module

Create a new module in `src/ive/src/`. The IVE follows a per-invariant module
structure:

```
src/ive/src/
├── liveness.rs            # Liveness verification
├── exclusivity.rs         # Exclusivity verification
├── interpretation.rs      # Interpretation verification
├── origin.rs              # Origin verification
├── cleanup.rs             # Cleanup verification
├── alignment.rs           # NEW: Alignment verification
├── invariant_aggregator.rs # Runs all passes and produces unified results
└── ...
```

The verification module should expose a verifier struct and a top-level
verification function:

```rust
// src/ive/src/alignment.rs

use vuma_core::msg::MSG;
use crate::result::VerificationResult;

/// Verifier for the alignment invariant.
///
/// Checks that every memory access in the MSG targets an address
/// that satisfies the alignment requirement of the access's RepD.
/// For example, a 64-bit access must target an 8-byte-aligned address.
pub struct AlignmentVerifier {
    verbose: bool,
}

impl AlignmentVerifier {
    pub fn new() -> Self { Self { verbose: false } }

    /// Verify alignment for all accesses in the MSG.
    pub fn verify(&self, msg: &MSG) -> VerificationResult {
        // For every access node in the MSG, check that the target address
        // satisfies the alignment requirement of the operation's RepD.
        // ...
    }
}
```

### 4.4 Step 4: Register the Module

Add `pub mod alignment;` to `src/ive/src/lib.rs` and add the re-exports:

```rust
pub mod alignment;

pub use alignment::{AlignmentVerifier, AlignmentViolation};
```

### 4.5 Step 5: Integrate into the Aggregator Pipeline

Add the new pass to the `InvariantAggregator` in
`src/ive/src/invariant_aggregator.rs`. The aggregator runs all verification
passes and produces a unified `VerificationSummary`:

```rust
impl InvariantAggregator {
    pub fn verify_all(&self, msg: &MSG) -> VerificationSummary {
        let mut results = Vec::new();

        results.push(self.verify_liveness(msg));
        results.push(self.verify_exclusivity(msg));
        results.push(self.verify_interpretation(msg));
        results.push(self.verify_origin(msg));
        results.push(self.verify_cleanup(msg));
        results.push(self.verify_alignment(msg));  // New

        VerificationSummary::from(results)
    }
}
```

### 4.6 Step 6: Add Proof Support (vuma-proof)

If the invariant can be formally proven, add a proof module in
`src/proof/src/`:

```rust
// src/proof/src/alignment_proofs.rs

/// Formal proof objects for alignment verification.
/// Constructed by the IVE when alignment can be proven statically.
pub struct AlignmentProof { /* ... */ }
```

Register the module in `src/proof/src/lib.rs` and add re-exports.

### 4.7 Step 7: Update the VUMA Core (vuma)

If the invariant requires new data in the MSG (e.g., alignment metadata on
regions), add it to `src/vuma/src/region.rs` or the relevant module.

### 4.8 Step 8: Add Tests and Examples

- **Unit tests** for the verification pass in the new module
- **Integration test** with a program that violates the invariant
- **Example program** in `examples/` demonstrating correct usage
- **Update existing tests** that check the full invariant set (the
  `InvariantAggregator` now returns 6 results instead of 5)
- **Counterexample tests** verifying that violations produce actionable diagnostics

### 4.9 Step 9: Update Documentation

- Add the invariant term to `docs/GLOSSARY.md` with a full definition,
  pronunciation, and cross-references
- Update `docs/CONVENTIONS.md` if any naming conventions are affected
- Update the IVE crate-level documentation in `src/ive/src/lib.rs`

---

## 6. How to Add New Backend Instructions

The codegen crate (`src/codegen/`) defines instructions for all 10 backends (AArch64, x86_64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32), register allocation, and the SCG → IR → machine code lowering pipeline. Adding a new instruction requires changes across several modules.

### 6.1 Step 1: Define the Instruction Variant

Add the instruction to the relevant backend's `Instruction` enum (e.g., `src/codegen/src/arm64.rs` for AArch64, `src/codegen/src/x86_64/mod.rs` for x86_64). Follow the existing pattern:

```rust
pub enum Instruction {
    // ... existing variants ...

    /// Atomic add: `LDADD Rs, Rt, [Rn]` (ARMv8.1-LSE)
    LDADD {
        /// Source register (value to add).
        rs: Register,
        /// Destination register (old value before add).
        rt: Register,
        /// Base register (memory address).
        rn: Register,
        /// Operand size (32 or 64 bit).
        size: OperandSize,
        /// Acquire semantics.
        acquire: bool,
        /// Release semantics.
        release: bool,
    },
}
```

### 6.2 Step 2: Implement the Encoding

Add the binary encoding in the `Instruction::encode()` method. Follow the
ARM Architecture Reference Manual (ARMv8-A) encoding tables:

```rust
impl Instruction {
    pub fn encode(&self) -> Result<u32> {
        match self {
            // ... existing encodings ...

            // LDADD: 1 0 111 0 00 ar 1 0 Rn Rs 0 o Rt
            // ar = acquire/release bits, o = size bit
            Instruction::LDADD { rs, rt, rn, size, acquire, release } => {
                let size_bit = match size {
                    OperandSize::Bit32 => 0,
                    OperandSize::Bit64 => 1,
                };
                let a_bit = if *acquire { 1 } else { 0 };
                let r_bit = if *release { 1 } else { 0 };
                Ok(0x38200000u32
                    | (size_bit << 31)
                    | (a_bit << 23)
                    | (r_bit << 22)
                    | (rn.encoding() << 16)
                    | (rs.encoding() << 10)
                    | rt.encoding())
            }
        }
    }
}
```

### 6.3 Step 3: Implement the Display Trait

Add the assembly output for the new instruction in the `Display` impl:

```rust
impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // ... existing arms ...
            Instruction::LDADD { rs, rt, rn, size, acquire, release } => {
                let suffix = match (*acquire, *release) {
                    (true, true) => "al",
                    (true, false) => "a",
                    (false, true) => "l",
                    (false, false) => "",
                };
                write!(f, "ldadd{} {}, {}, [{}]", suffix, rs, rt, rn)
            }
        }
    }
}
```

### 6.4 Step 4: Add the IR Instruction (if needed)

If the new instruction does not map to an existing IR instruction,
add one in `src/codegen/src/ir.rs`. The IR is the target-independent
intermediate representation between the SCG and machine code.

### 6.5 Step 5: Add the SCG-to-IR Mapping

In `src/codegen/src/scg_to_ir.rs`, add the lowering rule that maps SCG
node types to the new IR instruction. If the instruction only applies to
a specific backend, add the mapping in the instruction selector for that
backend (e.g., `src/codegen/src/arm64.rs`, `src/codegen/src/x86_64/mod.rs`).

### 6.6 Step 6: Add Tests

- **Encoding test**: Verify the output bytes match the ARM Architecture
  Reference Manual. Use concrete register assignments and compare the
  32-bit encoding against the expected value:

  ```rust
  #[test]
  fn ldadd_encode_matches_arm_reference() {
      let inst = Instruction::LDADD {
          rs: Register::X1,
          rt: Register::X2,
          rn: Register::X3,
          size: OperandSize::Bit64,
          acquire: true,
          release: true,
      };
      let encoded = inst.encode().unwrap();
      assert_eq!(encoded, 0xF8E06841); // Expected per ARM spec
  }
  ```

- **Integration test**: Build a small SCG program using the new instruction,
  run codegen, and verify the assembly output.
- **QEMU test**: If available, run the generated binary under QEMU and
  verify correct execution.

### 6.7 Step 7: Update Documentation

- Add the instruction to the codegen crate's module-level documentation
- If the instruction affects VUMA invariants (e.g., atomic instructions
  affect exclusivity verification, barrier instructions affect memory
  ordering verification), update the relevant spec documents in `docs/specs/`
- Add the instruction mnemonic to `docs/GLOSSARY.md` if it is a significant
  addition (e.g., a new atomic or barrier instruction)

---

## 7. Code Review Process

### 7.1 Before Submitting

1. **Run the full CI check locally** (see Section 2.7)
2. **Verify that no new `unsafe` blocks** are introduced without
   `// VUMA-VERIFIED` or `// IVE-TODO` annotations
3. **Update documentation** if your change affects public APIs, invariants,
   or the glossary
4. **Write tests** for all new functionality — unit tests, integration tests,
   and verification tests as appropriate
5. **Organize commits** into logical units following conventional commits
   (see `CONVENTIONS.md`)

### 7.2 Review Criteria

Reviewers will evaluate PRs against the following criteria:

1. **Correctness**: Does the code do what it claims? Are edge cases handled?
   Are there off-by-one errors in pointer arithmetic or graph traversal?
2. **Verification compliance**: Are all memory operations properly annotated?
   Does the IVE pass structure make sense? Does the new verification pass
   correctly identify violations and produce useful counterexamples?
3. **Documentation**: Are public items documented with doc comments? Are
   examples included? Is the glossary updated?
4. **Testing**: Is there adequate test coverage? Do tests cover failure modes
   and boundary conditions? Are property-based tests used where appropriate?
5. **Conventions**: Does the code follow the naming, formatting, and
   organizational conventions in `CONVENTIONS.md`?
6. **Performance**: Does the change introduce any performance regressions?
   Are allocations minimized in hot paths (especially IVE verification passes)?
7. **Architecture**: Does the change fit cleanly into the existing crate
   structure, or does it require restructuring? Are cross-crate dependencies
   maintained correctly (no circular dependencies)?

### 7.3 Review Timeline

- Initial review within **2 business days**
- Follow-up reviews within **1 business day**
- At least **1 approving review** required for merge
- Changes to IVE verification logic or VUMA invariants require **2 approving
  reviews** from maintainers with domain expertise

### 7.4 Special Review Rules

- **New SCG node types**: Must include IVE verification and codegen support
  in the same PR (no partial implementations)
- **New verification passes**: Must include a formal specification document
  in `docs/specs/` and proof infrastructure in `src/proof/`
- **New backend instructions**: Must include encoding tests verified against
  the architecture reference manual (ARM, x86, RISC-V, etc.)
- **Changes to `unsafe` annotations**: Any change to `// VUMA-VERIFIED` or
  `// IVE-TODO` annotations requires explicit reviewer acknowledgment
- **FFI/syscall changes**: Must test across all 10 backends with ABI conformance tests

---

## 8. PR Template

Every PR must use the following template. Copy it into the PR description
when opening a pull request.

```markdown
## Summary
[Brief description of changes — what and why]

## Related Issues
[Closes #X, Related to #Y]

## Changes
- [Specific change 1]
- [Specific change 2]
- [etc.]

## Verification Impact
- [ ] No new IVE-TODOs introduced
- [ ] All existing IVE-TODOs resolved or documented
- [ ] New SCG node types: [list or "none"]
- [ ] New VUMA invariants: [list or "none"]
- [ ] New ARM64 instructions: [list or "none"]
- [ ] Verification pass changes: [describe or "none"]

## Test Plan
- [Unit tests added/modified: describe]
- [Integration tests added/modified: describe]
- [Verification tests added/modified: describe]
- [Manual testing performed: describe]

## Checklist
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo test -p vuma-ive -- --test-threads=1 -- verification` passes
- [ ] All public items have doc comments with examples
- [ ] No bare `unsafe` blocks without `// VUMA-VERIFIED` or `// IVE-TODO` annotations
- [ ] `docs/GLOSSARY.md` updated if new terms introduced
- [ ] `docs/CONVENTIONS.md` updated if new patterns introduced
- [ ] Formal spec in `docs/specs/` if new invariant added
```

---

## Getting Help

- **Documentation**: Start with `docs/GLOSSARY.md` for terminology and
  `docs/CONVENTIONS.md` for coding standards
- **Architecture**: See `docs/ROADMAP.md` for the project structure and plan
- **Specifications**: See `docs/specs/` for formal specifications of VUMA
  invariants, algorithms, and the ARM64 codegen
- **Issues**: File bugs or request features via GitHub Issues
- **Discussions**: Use GitHub Discussions for design questions and proposals

---

*This guide is a living document. If you find errors, ambiguities, or missing
information, please submit a PR to improve it.*
