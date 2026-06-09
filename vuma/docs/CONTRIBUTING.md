# Contributing to VUMA

Thank you for contributing to the VUMA project — the Verified-Unsafe Memory Access framework for AI-native programming language design. This guide explains how to set up your development environment, make changes, and get them merged.

---

## 1. Development Environment Setup

### 1.1 Prerequisites

| Tool | Minimum Version | Purpose |
|------|----------------|---------|
| Rust toolchain (nightly) | nightly-2024-01-01 or later | Core language; VUMA uses nightly features |
| `rustup` | Latest | Rust toolchain management |
| `cargo` | Included with Rust | Build system |
| Git | 2.30+ | Version control |
| QEMU (aarch64) | 7.0+ | ARM64 emulation for testing without Pi 5 hardware |
| Raspberry Pi 5 | Any RAM config | Physical target (optional but recommended) |

### 1.2 Install the Rust Toolchain

```bash
# Install rustup if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add the ARM64 target
rustup target add aarch64-unknown-none

# Install nightly toolchain (required for VUMA's advanced features)
rustup toolchain install nightly
rustup default nightly

# Install required components
rustup component add rustfmt clippy rust-src llvm-tools
```

### 1.3 Clone and Build

```bash
git clone https://github.com/vuma-project/vuma.git
cd vuma

# Build all workspace members
cargo build --workspace

# Run all tests
cargo test --workspace

# Run clippy
cargo clippy --workspace -- -D warnings

# Check formatting
cargo fmt --check
```

### 1.4 Pi 5 Cross-Compilation (Optional)

If you are developing on an x86_64 host and targeting the Pi 5:

```bash
# Add the ARM64 Linux target for userspace testing
rustup target add aarch64-unknown-linux-gnu

# Install the ARM64 linker
# On Ubuntu/Debian:
sudo apt install gcc-aarch64-linux-gnu

# Build for ARM64
cargo build --target aarch64-unknown-linux-gnu
```

For bare-metal Pi 5 targets (no OS), use `aarch64-unknown-none` and consult `src/pi5/README.md` for linker script configuration.

### 1.5 QEMU Setup for ARM64 Testing

```bash
# Install QEMU with ARM64 support
# On Ubuntu/Debian:
sudo apt install qemu-system-arm

# Run a test binary under QEMU
qemu-aarch64 -L /usr/aarch64-linux-gnu target/aarch64-unknown-linux-gnu/debug/vuma-test
```

---

## 2. Running Tests

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
```

Run a specific test:

```bash
cargo test -p vuma-ive -- verify_liveness_catches_use_after_free
```

### 2.2 Integration Tests

Integration tests live in `src/tests/` and exercise cross-crate workflows:

```bash
cargo test -p vuma-tests
```

### 2.3 Verification Tests

The IVE has its own verification test suite that exercises the full verification pipeline:

```bash
# Run IVE verification tests (may be slow)
cargo test -p vuma-ive -- --test-threads=1 -- verification

# Run with verbose output to see verification details
cargo test -p vuma-ive -- --nocapture -- verification
```

### 2.4 Codegen Tests

ARM64 codegen tests produce assembly output that is compared against expected outputs:

```bash
cargo test -p vuma-codegen

# Update expected assembly snapshots (use with caution)
cargo test -p vuma-codegen -- -- snapshots --update
```

### 2.5 Full CI Check

Before submitting a PR, run the full CI check locally:

```bash
cargo fmt --check &&
cargo clippy --workspace -- -D warnings &&
cargo test --workspace &&
cargo test -p vuma-ive -- --test-threads=1 -- verification
```

---

## 3. How to Add a New SCG Node Type

The Semantic Computation Graph is extensible. Adding a new node type involves changes across three crates.

### 3.1 Step 1: Define the Node Type (vuma-scg)

Create a new file in `src/scg/src/nodes/` (or add to an existing file if the node is small):

```rust
// src/scg/src/nodes/atomic_swap_node.rs

use crate::Node;
use crate::annotations::Annotation;

/// An atomic swap operation node in the SCG.
///
/// Represents an atomic compare-and-swap that reads a value from memory,
/// compares it against an expected value, and conditionally writes a new value.
/// Maps to the ARM64 LDXR/STXR instruction pair.
///
/// # SCG Semantics
///
/// - Input edges: address, expected_value, new_value
/// - Output edges: old_value, success_flag
/// - Effects: memory read, conditional memory write
/// - Invariants: exclusivity must be verified for the address
pub struct AtomicSwapNode {
    /// Unique node identifier within the SCG
    pub id: NodeId,
    /// The address being atomically accessed
    pub address: EdgeRef,
    /// The expected current value
    pub expected: EdgeRef,
    /// The value to write if the comparison succeeds
    pub new_value: EdgeRef,
    /// Node annotations (type info, constraints, verification status)
    pub annotations: Vec<Annotation>,
}
```

### 3.2 Step 2: Register the Node Variant

Add the variant to the `NodeKind` enum in `src/scg/src/node_kind.rs`:

```rust
pub enum NodeKind {
    // ... existing variants ...
    AtomicSwap,
}
```

And implement the `Node` trait for `AtomicSwapNode`, or add it to the `Node` enum if using an enum-based approach.

### 3.3 Step 3: Add IVE Verification (vuma-ive)

Add a verification function for the new node in `src/ive/src/passes/`:

```rust
// src/ive/src/passes/atomic_swap_verification.rs

use vuma_ive::{IveError, VerificationResult};
use vuma_scg::AtomicSwapNode;
use vuma_vuma::MemoryStateGraph;

/// Verify that an atomic swap node respects all VUMA invariants.
///
/// Checks:
/// - Liveness: the target address is allocated
/// - Exclusivity: the LDXR/STXR pair provides hardware-enforced exclusivity
/// - Interpretation: the address points to a region whose RepD matches the value type
/// - Origin: the address traces back to a valid allocation
pub fn verify_atomic_swap(
    node: &AtomicSwapNode,
    msg: &MemoryStateGraph,
) -> Result<VerificationResult, IveError> {
    // Implementation here
}
```

### 3.4 Step 4: Add Codegen (vuma-codegen)

Add the code generation rule in `src/codegen/src/arm64/`:

```rust
// src/codegen/src/arm64/atomic_ops.rs

use vuma_scg::AtomicSwapNode;

/// Emit ARM64 assembly for an atomic swap node.
///
/// Generates the LDXR/STXR loop pattern:
/// ```asm
/// .L_retry_{id}:
///     ldxr w{tmp}, [x{addr}]
///     cmp w{tmp}, w{expected}
///     b.ne .L_fail_{id}
///     stxr w{status}, w{new}, [x{addr}]
///     cbnz w{status}, .L_retry_{id}
/// ```
pub fn emit_atomic_swap(node: &AtomicSwapNode, ctx: &mut CodegenContext) -> String {
    // Implementation here
}
```

### 3.5 Step 5: Add Tests

Add unit tests in each crate and at least one integration test in `src/tests/` that exercises the full pipeline: SCG construction → IVE verification → codegen → assembly output check.

---

## 4. How to Add a New VUMA Invariant

VUMA currently defines five global invariants (liveness, exclusivity, interpretation, origin, cleanup). Adding a new invariant requires careful design and cross-crate coordination.

### 4.1 Step 1: Define the Invariant

Add the invariant to the `VumaInvariant` enum in `src/vuma/src/invariants.rs`:

```rust
pub enum VumaInvariant {
    Liveness,
    Exclusivity,
    Interpretation,
    Origin,
    Cleanup,
    // New invariant
    Alignment,  // Every access is properly aligned for its RepD
}
```

### 4.2 Step 2: Write the Specification

Create a formal specification document in `docs/specs/` describing the invariant:

- **Definition**: What property does the invariant guarantee?
- **Scope**: Which SCG nodes and MSG edges does it apply to?
- **Verification algorithm**: How does the IVE check this invariant? What is the time complexity?
- **Counterexample format**: When verification fails, what information is reported?
- **Relationship to existing invariants**: Is the new invariant independent, implied by, or does it imply any existing invariant?

### 4.3 Step 3: Implement Verification

Add a new verification pass in `src/ive/src/passes/`:

```rust
pub fn verify_alignment(msg: &MemoryStateGraph) -> Result<AlignmentReport, IveError> {
    // For every access node in the MSG, check that the target address
    // satisfies the alignment requirement of the operation's RepD.
    // ...
}
```

### 4.4 Step 4: Integrate into the IVE Pipeline

Add the new pass to the IVE's verification pipeline in `src/ive/src/pipeline.rs`:

```rust
pub fn run_verification(msg: &MemoryStateGraph) -> VerificationSummary {
    let mut results = Vec::new();

    results.push(verify_liveness(msg));
    results.push(verify_exclusivity(msg));
    results.push(verify_interpretation(msg));
    results.push(verify_origin(msg));
    results.push(verify_cleanup(msg));
    results.push(verify_alignment(msg));  // New

    VerificationSummary::from(results)
}
```

### 4.5 Step 5: Update the Glossary

Add the new invariant term to `docs/GLOSSARY.md` with a full definition, pronunciation, and cross-references.

### 4.6 Step 6: Add Tests and Examples

- Unit tests for the verification pass
- Integration test with a program that violates the invariant
- Example program in `examples/` demonstrating correct usage
- Update existing tests that check the full invariant set

---

## 5. How to Add a New ARM64 Instruction to Codegen

### 5.1 Step 1: Define the Instruction Type

Add the instruction to `src/codegen/src/arm64/instructions.rs`:

```rust
pub enum Arm64Instruction {
    // ... existing instructions ...
    /// CAS - Compare and Swap word (ARMv8.1-LSE)
    Cas {
        size: OperandSize,   // 32 or 64 bit
        rs: Register,        // Source (new value)
        rt: Register,        // Expected value / result
        rn: Register,        // Memory address
        acquire: bool,       // Acquire semantics
        release: bool,       // Release semantics
    },
}
```

### 5.2 Step 2: Implement Encoding

Add the binary encoding in `src/codegen/src/arm64/encoder.rs`:

```rust
impl Arm64Instruction {
    pub fn encode(&self) -> Result<u32, CodegenError> {
        match self {
            // ... existing encodings ...
            Arm64Instruction::Cas { size, rs, rt, rn, acquire, release } => {
                let size_bit = match size {
                    OperandSize::Bit32 => 0,
                    OperandSize::Bit64 => 1,
                };
                // ARMv8.1 CAS encoding: 0 0 001000 L o 1 R s 0 Rn Rt
                let opcode = 0x08A0_0000
                    | (size_bit << 31)
                    | ((*acquire as u32) << 15)
                    | ((*release as u32) << 17)
                    | (rn.encode() << 5)
                    | rt.encode();
                Ok(opcode)
            }
        }
    }
}
```

### 5.3 Step 3: Add the SCG-to-Instruction Mapping

In `src/codegen/src/arm64/lower.rs`, add the lowering rule that maps SCG node types to the new instruction:

```rust
fn lower_node(node: &Node, ctx: &mut LoweringContext) -> Vec<Arm64Instruction> {
    match node.kind() {
        // ... existing mappings ...
        NodeKind::AtomicSwap => {
            vec![Arm64Instruction::Cas {
                size: OperandSize::Bit32,
                rs: ctx.alloc_reg(),
                rt: ctx.alloc_reg(),
                rn: ctx.alloc_reg(),
                acquire: true,
                release: true,
            }]
        }
    }
}
```

### 5.4 Step 4: Add Tests

- Unit test for encoding: verify the output bytes match the ARM Architecture Reference Manual
- Integration test: build a small SCG program using the new instruction, run codegen, and verify the assembly output
- If using QEMU, run the generated binary and verify correct execution

### 5.5 Step 5: Update Documentation

- Add the instruction to the codegen README
- If the instruction affects VUMA invariants (e.g., barrier instructions affect exclusivity verification), update the relevant spec documents

---

## 6. How to Add a New Example Program

Example programs demonstrate VUMA features and serve as integration test cases.

### 6.1 Step 1: Create the Example

Create a new file in `examples/`:

```
examples/
├── hello_uart/           # Minimal UART output
├── linked_list/          # Doubly-linked list (VUMA-VERIFIED, no unsafe)
├── atomic_counter/       # Lock-free atomic counter using LDXR/STXR
├── gpio_blink/           # GPIO LED blink on Pi 5
└── your_new_example/     # Your new example
    ├── main.rs           # Example program
    └── README.md         # Description and expected output
```

### 6.2 Step 2: Write the Example

Follow the VUMA conventions (see `CONVENTIONS.md`). Every example must:

1. Include a `main.rs` with a `fn main()` entry point
2. Be annotated with `// VUMA-VERIFIED` or `// IVE-TODO` for all memory operations
3. Include a `README.md` explaining what the example demonstrates and what VUMA features it uses
4. Compile for the `aarch64-unknown-none` target (bare-metal Pi 5)

### 6.3 Step 3: Add the Cargo Configuration

Add the example to the workspace `Cargo.toml` or the relevant crate's `Cargo.toml`:

```toml
[[example]]
name = "your_new_example"
path = "examples/your_new_example/main.rs"
```

### 6.4 Step 4: Add Integration Tests

Add a test in `src/tests/` that builds the example, runs IVE verification, and optionally executes it under QEMU:

```rust
#[test]
fn example_your_new_example_verification_passes() {
    let scg = build_example_scg("your_new_example");
    let msg = vuma_vuma::MemoryStateGraph::from_scg(&scg);
    let result = vuma_ive::run_verification(&msg);
    assert!(result.all_passed(), "Verification failed: {:?}", result.failures());
}
```

---

## 7. PR Review Process

### 7.1 Before Submitting

1. **Run the full CI check locally** (see Section 2.5)
2. **Verify that no new `unsafe` blocks** are introduced without `// VUMA-VERIFIED` or `// IVE-TODO` annotations
3. **Update documentation** if your change affects public APIs, invariants, or the glossary
4. **Write tests** for all new functionality
5. **Squash or organize commits** into logical units following conventional commits

### 7.2 PR Template

Every PR must include:

```markdown
## Summary
[Brief description of changes]

## Related Issues
[Closes #X, Related to #Y]

## Changes
- [Specific change 1]
- [Specific change 2]

## Verification Impact
- [ ] No new IVE-TODOs introduced
- [ ] All existing IVE-TODOs resolved or documented
- [ ] New SCG node types: [list or "none"]
- [ ] New VUMA invariants: [list or "none"]
- [ ] Verification pass changes: [describe or "none"]

## Test Plan
- [Unit tests added/modified]
- [Integration tests added/modified]
- [Manual testing performed]

## Checklist
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] All public items have doc comments
- [ ] No bare `unsafe` blocks without annotations
- [ ] GLOSSARY.md updated if new terms introduced
```

### 7.3 Review Criteria

Reviewers will check:

1. **Correctness**: Does the code do what it claims? Are edge cases handled?
2. **Verification compliance**: Are all memory operations properly annotated? Does the IVE pass structure make sense?
3. **Documentation**: Are public items documented? Are examples included?
4. **Testing**: Is there adequate test coverage? Do tests cover failure modes?
5. **Conventions**: Does the code follow the naming, formatting, and organizational conventions in `CONVENTIONS.md`?
6. **Performance**: Does the change introduce any performance regressions? Are allocations minimized in hot paths?
7. **Architecture**: Does the change fit cleanly into the existing crate structure, or does it require restructuring?

### 7.4 Review Timeline

- Initial review within 2 business days
- Follow-up reviews within 1 business day
- At least 1 approving review required for merge
- Changes to IVE verification logic or VUMA invariants require 2 approving reviews

---

## 8. Verification Checklist Before Merge

Before any PR is merged, the following checklist must be satisfied. This checklist is enforced by CI and by reviewer judgment.

### 8.1 Automated Checks (CI)

- [ ] `cargo fmt --check` — all code is formatted
- [ ] `cargo clippy --workspace -- -D warnings` — no clippy warnings
- [ ] `cargo test --workspace` — all tests pass
- [ ] `cargo test -p vuma-ive -- --test-threads=1 -- verification` — IVE verification tests pass
- [ ] No bare `unsafe` blocks without `// VUMA-VERIFIED` or `// IVE-TODO` annotations
- [ ] No new `// IVE-TODO` items without a tracking issue

### 8.2 Manual Checks (Reviewers)

- [ ] Public API changes are documented with examples
- [ ] New SCG node types have corresponding IVE verification passes
- [ ] New VUMA invariants have formal specifications in `docs/specs/`
- [ ] New ARM64 instructions have correct encoding (verified against ARM Architecture Reference Manual)
- [ ] The change does not introduce circular dependencies between workspace crates
- [ ] Performance-sensitive code has been profiled (especially IVE verification passes and codegen)
- [ ] The PR description accurately describes the change

### 8.3 Post-Merge

After merging:

1. Monitor CI on `main` for any failures
2. If the change affects the IVE, run the full verification suite on the benchmark programs
3. If the change affects codegen, verify that the Pi 5 boot sequence still works (on QEMU or hardware)
4. Update the changelog with the merged change

---

## 9. Getting Help

- **Documentation**: Start with `docs/GLOSSARY.md` for terminology and `docs/CONVENTIONS.md` for coding standards
- **Architecture**: See `docs/ROADMAP.md` for the project structure and plan
- **Issues**: File bugs or request features via GitHub Issues
- **Discussions**: Use GitHub Discussions for design questions and proposals

---

*This guide is a living document. If you find errors, ambiguities, or missing information, please submit a PR to improve it.*
