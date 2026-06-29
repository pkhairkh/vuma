# VUMA Coding Conventions

This document defines the coding standards and conventions for the VUMA project.
All contributors must follow these conventions to maintain consistency across the
codebase and to support the IVE verification workflow.

---

## Table of Contents

1. [Rust Coding Style (Beyond rustfmt)](#1-rust-coding-style-beyond-rustfmt)
2. [Error Handling Patterns](#2-error-handling-patterns)
3. [Testing Conventions](#3-testing-conventions)
4. [Naming Conventions for VUMA Types](#4-naming-conventions-for-vuma-types)
5. [Documentation Conventions](#5-documentation-conventions)
6. [Git Commit Message Format](#6-git-commit-message-format)

---

## 1. Rust Coding Style (Beyond rustfmt)

### 1.1 Base Standard

All VUMA Rust code follows `rustfmt` with project-specific settings defined in
`rustfmt.toml`. Run `cargo fmt` before every commit. The CI pipeline will reject
any PR that does not pass `cargo fmt --check`.

Project-specific `rustfmt.toml` settings:

```toml
max_width = 100        # Wider than default 100 — but doc comments are 80
tab_spaces = 4
edition = "2021"
```

Beyond `rustfmt` defaults, we enforce these additional rules:

- **Maximum line length**: 100 characters for code, **80 characters for doc
  comments**. Doc comments that exceed 80 characters must be wrapped. This
  ensures readability in `cargo doc` output and terminal windows.
- **Imports**: Group imports in the following order, separated by blank lines:
  1. Standard library (`use std::...`)
  2. External crates (`use serde::...`, `use petgraph::...`)
  3. Internal crates (`use vuma_scg::...`, `use vuma_ive::...`)
  4. Current crate (`use crate::...`)
- **Trailing commas**: Always use trailing commas in multi-line constructs
  (function args, struct fields, enum variants, match arms).
- **Match arms**: Always use `{}` blocks for match arms, even single-expression
  arms. This makes it easier to add debugging or logging later:

  ```rust
  // PREFERRED
  match node_type {
      NodeType::Computation => {
          process_computation(node)
      }
      NodeType::Allocation => {
          process_allocation(node)
      }
  }

  // AVOID
  match node_type {
      NodeType::Computation => process_computation(node),
      NodeType::Allocation => process_allocation(node),
  }
  ```

- **Implicit returns**: Prefer implicit returns (omitting the semicolon on the
  final expression) for short functions. Use explicit `return` only for early
  returns.
- **Struct literal formatting**: Use block formatting for struct literals when
  any field has a long value or when there are more than two fields.
- **Method chaining**: Break long method chains so that each method call is on
  its own line, aligned under the dot:

  ```rust
  let result = scg
      .nodes()
      .filter(|n| n.node_type == NodeType::Access)
      .map(|n| verify_access(n, msg))
      .collect::<Vec<_>>();
  ```

### 1.2 Clippy

All code must pass `cargo clippy` with no warnings. The CI pipeline runs clippy
with `-D warnings`, so any clippy warning is treated as an error. The project
uses `clippy.toml` with `cognitive-complexity-threshold = 50` — any function
exceeding this threshold should be refactored into smaller functions.

If a clippy lint is genuinely inappropriate for a specific case, add a targeted
`#[allow(clippy::lint_name)]` with a comment explaining why:

```rust
// This match is exhaustive by construction; the wildcard is a safety net.
#[allow(clippy::match_wildcard_for_single_variants)]
match result {
    VerificationStatus::ProvenSafe => { /* ... */ }
    _ => { /* handle all other cases uniformly */ }
}
```

### 1.3 Cognitive Complexity

The project enforces a cognitive complexity threshold of 50 (configured in
`clippy.toml`). Functions that exceed this threshold must be refactored. Common
refactoring strategies:

- Extract helper functions for nested conditionals
- Use early returns to reduce nesting
- Replace complex match arms with helper methods
- Use the builder pattern for complex construction

### 1.4 Type Safety and Newtypes

Use newtype wrappers for domain-specific identifiers to prevent accidental
confusion between different kinds of IDs. The project follows this pattern
extensively:

```rust
/// Unique identifier for a node within the SCG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

/// Unique identifier for an edge within the SCG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub u64);

/// Unique identifier for a region within the SCG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionId(pub u64);
```

This prevents bugs like accidentally passing an `EdgeId` where a `NodeId` is
expected, which would compile fine with raw `u64` values.

### 1.5 Derive Macros

All data-carrying types should derive at minimum: `Debug`, `Clone`. For types
that are used as map keys or in equality comparisons, also derive `PartialEq`,
`Eq`, `Hash`. For types that need serialization, derive `Serialize`,
`Deserialize` from serde.

---

## 2. Error Handling Patterns

### 2.1 Library Crates — Use `thiserror`

All error types in library crates (`vuma-scg`, `vuma-ive`, `vuma-bd`,
`vuma-codegen`, `vuma-core`, `vuma-proof`, `vuma-cor`, `vuma-parser`,
`vuma-std`) must use **`thiserror`** for error
definitions. This ensures that error types are concrete, implement
`std::error::Error`, and have useful `Display` implementations.

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum IveError {
    #[error("liveness verification failed for access at {address:#x}")]
    LivenessViolation {
        address: u64,
        path: Vec<String>,
    },

    #[error("exclusivity violation: concurrent write detected at {address:#x}")]
    ExclusivityViolation {
        address: u64,
        thread_a: String,
        thread_b: String,
    },

    #[error("verification debt limit exceeded: {pending} unverified properties")]
    VerificationDebtExceeded { pending: usize },

    #[error("encoding error: {0}")]
    Encoding(#[from] crate::codegen::CodegenError),
}
```

**Rules for error types:**

- Use `#[error("...")]` with a human-readable message that includes all relevant
  context (addresses, IDs, paths)
- Use `#[from]` for automatic `From` impls when wrapping errors from other crates
- Group related error variants together with a comment separator
- Never use `Box<dyn std::error::Error>` in library code — always use a concrete
  error enum
- Never use `String` as an error type — always create a named variant with
  structured fields

### 2.2 Application Crates — Use `anyhow`

Use **`anyhow`** for application-level error handling in CLI tools,
integration tests, and example programs. This allows flexible error chaining
and context attachment without defining exhaustive error enums.

```rust
use anyhow::{Context, Result};

fn run_verification(scg: &SemanticComputationGraph) -> Result<()> {
    let msg = scg
        .build_memory_state_graph()
        .context("failed to build Memory State Graph")?;
    ive::verify_liveness(&msg)
        .context("liveness verification failed")?;
    Ok(())
}
```

**Rules for `anyhow` usage:**

- Always use `.context("...")` or `.with_context(|| ...)` to attach a
  human-readable description of what was being attempted when the error occurred
- Chain context for a causal trace: the outermost context should describe the
  high-level operation, and inner contexts should describe the specific step
- Use `anyhow::Result<T>` as the return type, not `std::result::Result<T, anyhow::Error>`

### 2.3 No Panics in Library Code

Library crates must **never panic** in production code. Use `Result` for all
fallible operations. The only acceptable uses of `panic!`, `unwrap()`, or
`expect()` are:

1. **In test code** (`#[cfg(test)]` modules) — `unwrap()` and `expect()` are
   encouraged in tests for clarity
2. **For true programming errors** that indicate a bug in the caller — in this
   case, use `assert_eq!`, `assert!`, or `panic!` with a message explaining
   the invariant that was violated
3. **In `unreachable!()` branches** after exhaustive matches where the compiler
   cannot prove exhaustiveness
4. **In `todo!()` markers** for unimplemented functionality (these are acceptable
   during development but must not appear in released code)

**Anti-patterns to avoid:**

```rust
// BAD: Silent unwrap that hides the failure mode
let region = msg.region_of(address).unwrap();

// BAD: Indexing that can panic
let first = nodes[0];

// GOOD: Explicit error handling
let region = msg
    .region_of(address)
    .ok_or(IveError::RegionNotFound { address })?;

// GOOD: Bounds-checked access
let first = nodes
    .first()
    .ok_or(IveError::EmptyNodeList)?;
```

### 2.4 Error Propagation Across Crates

When errors propagate across crate boundaries, wrap them using `#[from]` or
explicit conversion:

```rust
// In vuma-ive (which depends on vuma-scg and vuma-codegen)
#[derive(Error, Debug)]
pub enum IveError {
    #[error("SCG error: {0}")]
    Scg(#[from] vuma_scg::ScgError),

    #[error("codegen error: {0}")]
    Codegen(#[from] vuma_codegen::CodegenError),
}
```

Never leak internal error types through public APIs. Each crate should define
its own error enum that wraps dependencies.

---

## 3. Testing Conventions

### 3.1 Test Categories

VUMA distinguishes three categories of tests, each serving a different purpose:

| Category | Location | Scope | Speed |
|----------|----------|-------|-------|
| **Unit** | `#[cfg(test)] mod tests` in each file | Single function or type | Fast (<1ms each) |
| **Integration** | `src/tests/` workspace crate | Cross-crate workflows | Medium (<100ms each) |
| **Verification** | IVE-specific test functions | Full verification pipeline | Slow (seconds each) |

### 3.2 Unit Tests

Unit tests live in the same file as the code they test, inside a
`#[cfg(test)] mod tests` block at the bottom of the file. Unit tests should be
focused, fast, and deterministic. Each test function name should describe the
specific scenario being tested, following the pattern
`{method}_{scenario}_{expected_outcome}`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_liveness_catches_use_after_free() {
        // Arrange: build a minimal MSG where an access targets a freed region
        // Act: run liveness verification
        // Assert: verification returns a LivenessViolation
    }

    #[test]
    fn verify_liveness_accepts_valid_access() {
        // Arrange: build a minimal MSG where all accesses target live regions
        // Act: run liveness verification
        // Assert: verification returns ProvenSafe
    }

    #[test]
    fn encode_ldr_unsigned_offset_produces_correct_machine_code() {
        let inst = Instruction::LDR {
            rt: Register::X0,
            rn: Register::X1,
            offset: 16,
        };
        let encoded = inst.encode().unwrap();
        assert_eq!(encoded, 0xF94080A0);
    }
}
```

**Unit test rules:**

- Use the Arrange-Act-Assert pattern (or Given-When-Then)
- Do not depend on external state, network, or filesystem
- Do not depend on execution order — each test must be independently runnable
- Avoid test interdependence; use fresh data structures for each test
- Prefer specific assertions over vague ones:
  - `assert_eq!(result.status, VerificationStatus::ProvenSafe)` ✓
  - `assert!(result.is_ok())` — too vague, use only when the specific value
    is not important

### 3.3 Integration Tests

Integration tests live in the `src/tests/` workspace crate (`vuma-tests`) for
cross-crate integration tests. Individual crates may also have a `tests/`
directory for crate-level integration tests.

Integration tests exercise complete workflows across crate boundaries:

```rust
// src/tests/src/dlist.rs — Integration test for doubly-linked list verification
#[test]
fn dlist_full_pipeline_verification_passes() {
    // 1. Parse the example program
    let scg = build_example_scg("doubly_linked_list");

    // 2. Convert SCG → MSG
    let msg = vuma_core::scg_to_msg::scg_to_msg(&scg).unwrap();

    // 3. Run all IVE verification passes
    let result = vuma_ive::InvariantAggregator::new().verify_all(&msg);

    // 4. Assert all invariants pass
    assert!(result.all_passed(), "Verification failures: {:?}", result.failures());
}
```

**Integration test rules:**

- Test complete workflows, not individual functions
- Cover the SCG → MSG → IVE → Codegen pipeline end-to-end
- Include both positive tests (correct programs pass verification) and negative
  tests (incorrect programs fail verification with expected diagnostics)
- Run under `--test-threads=1` when tests share global state

### 3.4 Verification Tests

Verification tests are IVE-specific tests that exercise the full verification
pipeline. They are slower than unit tests and may involve constructing large
MSG instances. These tests are tagged with `verification` for selective running:

```bash
# Run only verification tests
cargo test -p vuma-ive -- --test-threads=1 -- verification

# Run with verbose output
cargo test -p vuma-ive -- --nocapture -- verification
```

**Verification test rules:**

- Always run with `--test-threads=1` to avoid race conditions in shared state
- Test both the positive case (verification passes for correct programs) and
  the negative case (verification fails with actionable counterexamples for
  incorrect programs)
- Test each of the five invariants independently
- Test interactions between invariants (e.g., a program that passes liveness
  but fails exclusivity)

### 3.5 Property-Based Testing

For verification-related code, prefer property-based tests using the `proptest`
crate. This is especially important for the IVE, where edge cases in pointer
arithmetic and graph traversal are common:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn address_arithmetic_stays_in_region(
        base in 0x1000u64..0x10000u64,
        offset in 0u64..0x1000u64,
    ) {
        let region = Region::new(RegionId::new(1), Address::from(base), 0x1000);
        let addr = Address::from(base) + offset;
        prop_assert!(region.contains(&addr));
    }
}
```

### 3.6 Test Naming Convention

Test functions use `snake_case` and follow the pattern:
`{unit}_{scenario}_{expected_outcome}`

Examples:

- `verify_liveness_valid_access_returns_ok`
- `verify_liveness_freed_region_returns_violation`
- `codegen_ldxr_stxr_produces_valid_atomic`
- `encode_add_shifted_register_matches_arm_spec`
- `scg_to_msg_allocation_creates_region_with_monotonic_address`
- `bd_compatible_same_type_returns_true`
- `proof_checker_detects_circular_reasoning`

---

## 4. Naming Conventions for VUMA Types

### 4.1 General Rust Naming

| Item | Convention | Example |
|------|-----------|---------|
| Crates | `snake_case` | `vuma_scg`, `vuma_ive`, `vuma_codegen` |
| Modules | `snake_case` | `memory_model`, `graph_transform` |
| Types (struct, enum, trait) | `CamelCase` | `SemanticComputationGraph`, `BehavioralDescriptor` |
| Enum variants | `CamelCase` | `LivenessViolation`, `ExclusivityViolation` |
| Functions / methods | `snake_case` | `verify_liveness`, `infer_capabilities` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_VERIFICATION_DEPTH`, `DEFAULT_REGION_SIZE` |
| Static variables | `SCREAMING_SNAKE_CASE` | `GLOBAL_IVE_CONFIG` |
| Type parameters | Short `CamelCase` | `T`, `N` for common; `NodeKind`, `RepDescriptor` for domain |
| Lifetime parameters | Short lowercase | `'a`, `'ctx`, `'graph` |
| Macros | `snake_case!` | `scg_node!`, `define_bd!` |

### 4.2 VUMA-Specific Naming Conventions

These conventions apply to types and functions specific to the VUMA domain:

**SCG Node Types**: Named as `{Kind}Node` — the `Node` suffix is mandatory for
all node payload types in the SCG:

- `AllocationNode` — memory allocation operation
- `DeallocationNode` — memory deallocation operation
- `AccessNode` — memory read/write operation
- `ComputationNode` — pure computation
- `CastNode` — type conversion or coercion
- `EffectNode` — side-effecting operation
- `ControlNode` — control flow point
- `PhantomNode` — structural/analysis placeholder

**BD Components**: Named as `{DescriptorKind}Descriptor` in public APIs. The
abbreviations `RepD`, `CapD`, `RelD` are acceptable in doc comments, local
variables, and internal discussion, but **never** in public API names:

```rust
// Public API: use full names
pub struct RepresentationDescriptor { /* ... */ }
pub struct CapabilityDescriptor { /* ... */ }
pub struct RelationalDescriptor { /* ... */ }

// Doc comments and local variables: abbreviations OK
/// Computes the RepD (Representation Descriptor) for a pointer type.
fn compute_ptr_repd(pointee: &RepresentationDescriptor) -> RepresentationDescriptor {
    // ...
}
```

**Verification Result Types**: Named as `{Property}Result` or `{Property}Report`:

- `LivenessResult` / `LivenessReport`
- `ExclusivityResult` / `ExclusivityReport`
- `InterpretationResult` / `InterpretationReport`
- `VerificationSummary` — aggregated result from all passes
- `CounterExample` — structured failure description

**Verification Violation Types**: Named as `{Property}Violation`:

- `LivenessViolation`
- `ExclusivityViolation`
- `InterpretationViolation`
- `OriginViolation`
- `CleanupViolation`

**Error Types**: Named as `{Domain}Error` — the `Error` suffix is mandatory:

- `ScgError` — errors in SCG construction or validation
- `IveError` — errors in IVE verification or inference
- `CodegenError` — errors in ARM64 code generation
- `ConversionError` — errors in SCG → MSG conversion
- `ProofError` — errors in proof construction or checking

**Verifier Types**: Named as `{Property}Verifier`:

- `LivenessVerifier`
- `ExclusivityVerifier`
- `InterpretationVerifier`
- `CleanupVerifier`
- `AlignmentVerifier`

**ARM64 Instruction Types**: Named exactly as the ARM64 mnemonic in `CamelCase`:

- `Ldxr` — Load Exclusive Register
- `Stxr` — Store Exclusive Register
- `Dmb` — Data Memory Barrier
- `Dsb` — Data Synchronization Barrier
- `Isb` — Instruction Synchronization Barrier
- `Cas` — Compare and Swap

**Identifier Newtypes**: Named as `{Entity}Id` — the `Id` suffix is mandatory:

- `NodeId` — unique node identifier in the SCG
- `EdgeId` — unique edge identifier in the SCG
- `RegionId` — unique region identifier
- `AccessId` — unique access identifier
- `DerivationId` — unique derivation identifier
- `SyncEdgeId` — unique synchronization edge identifier

### 4.3 Function Naming Patterns

| Pattern | Meaning | Example |
|---------|---------|---------|
| `verify_{property}` | Run a verification pass | `verify_liveness`, `verify_exclusivity` |
| `infer_{property}` | Infer a property from structure | `infer_capabilities`, `infer_representation` |
| `build_{entity}` | Construct a complex object | `build_memory_state_graph`, `build_derivation_chain` |
| `compute_{value}` | Derive a computed value | `compute_dominators`, `compute_hot_paths` |
| `find_{entities}` | Search and return results | `find_use_after_free`, `find_derivation_chains` |
| `emit_{output}` | Generate output (codegen) | `emit_atomic_swap`, `emit_barrier` |
| `lower_{source}` | Lower to a lower-level representation | `lower_to_arm64` |
| `encode` | Binary encode an instruction | `instruction.encode()` |
| `as_{type}` | Cheap reference conversion | `as_reg()`, `as_u64()` |
| `to_{type}` | Expensive owned conversion | `to_string()`, `to_scg()` |
| `is_{predicate}` | Boolean predicate | `is_callee_saved()`, `is_null()`, `is_valid()` |
| `has_{property}` | Boolean property check | `has_capability()`, `has_snapshot()` |

---

## 5. Documentation Conventions

### 5.1 Doc Comments Are Mandatory

**All public items** (functions, types, traits, modules, constants) must have
doc comments (`///` or `//!`). Doc comments must include:

1. **A summary line** (imperative mood, no trailing period):
   `/// Verify liveness of all memory accesses`
2. **A detailed description** (one or more paragraphs explaining the what,
   why, and how)
3. **At least one example** for functions and methods showing typical usage
4. **Panics** section if the function can panic (explain under what conditions)
5. **Errors** section if the function returns `Result` (describe what errors
   can be returned)
6. **Safety** section for any `// VUMA-VERIFIED` code (explain what the IVE
   verified)

Example:

```rust
/// Verify liveness of all memory accesses in the Memory State Graph
///
/// Traverses every access node in the MSG and checks that the target region
/// is allocated at the program point where the access occurs. This is the
/// first of the five VUMA global invariants (see GLOSSARY.md for details).
///
/// The verification algorithm performs a forward dataflow analysis over the
/// MSG, tracking the allocation status of each region at every program point.
/// Time complexity is O(N + E) where N is the number of nodes and E is the
/// number of edges in the MSG.
///
/// # Examples
///
/// ```
/// use vuma_ive::verify_liveness;
/// use vuma_core::msg::MSG;
///
/// let msg = MSG::from_scg(&scg);
/// let result = verify_liveness(&msg);
/// assert!(result.is_ok(), "all accesses target live memory");
/// ```
///
/// # Errors
///
/// Returns `IveError::LivenessViolation` if any access targets a freed
/// or unallocated region, with the offending address and execution path.
pub fn verify_liveness(msg: &MSG) -> Result<LivenessReport, IveError> {
    // ...
}
```

### 5.2 Module-Level Documentation

Every crate must have a `//!` module-level doc comment in its `lib.rs` (or
`main.rs`) that explains:

1. What the crate does
2. How it fits into the VUMA architecture
3. Key types and their relationships
4. Links to related crates
5. A quick-start code example

Example (from `src/scg/src/lib.rs`):

```rust
//! # VUMA SCG — Semantic Computation Graph
//!
//! This crate provides the core data structures and algorithms for the
//! **Semantic Computation Graph (SCG)**, a central component of the VUMA
//! framework for verified-unsafe memory access.
//!
//! ## Overview
//!
//! The SCG models program semantics as a directed graph where:
//! - **Nodes** represent operations (computation, allocation, …)
//! - **Edges** represent relationships between operations
//! - **Regions** group nodes into memory scopes
//!
//! ## Quick Start
//!
//! ```
//! use vuma_scg::SCG;
//! let mut scg = SCG::new();
//! ```
```

### 5.3 Internal Documentation

Private functions and types should also have doc comments when their purpose
is not immediately obvious from the name. A good rule of thumb: if you would
need to read the function body to understand what it does, it needs a doc
comment.

### 5.4 Doc Comment Line Length

Doc comments must not exceed **80 characters** per line. This ensures
readability in `cargo doc` output, terminal windows, and code review tools.
Break lines at natural boundaries (between sentences, after commas):

```rust
/// Verify that every memory access targets a region that is allocated at
/// that program point, eliminating use-after-free bugs by proving that no
/// execution path leads to a dereference of a freed or unallocated region.
```

### 5.5 Code Examples in Doc Comments

All code examples in doc comments must be valid Rust code that compiles and
runs. Use ````rust` fences for runnable examples and ````text` or ````asm`
for non-Rust output (assembly, hex dumps, etc.).

Prefer showing the common case over the edge case. Include assertions that
verify the expected behavior:

```rust
/// # Examples
///
/// ```
/// use vuma_bd::descriptor::BD;
/// use vuma_bd::repd::RepD;
/// use vuma_bd::capd::CapD;
///
/// let bd = BD::new(RepD::u64(), CapD::read_write(), RelD::empty());
/// assert!(bd.capd().can_read());
/// assert!(bd.capd().can_write());
/// ```
```

---

## 6. Git Commit Message Format

### 6.1 Conventional Commits

All commit messages must follow the [Conventional Commits](https://www.conventionalcommits.org/)
specification:

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

**Format rules:**

- The **type** and **scope** are lowercase
- The **description** is in imperative mood, lowercase, no trailing period
  ("add liveness pass" not "Added liveness pass")
- The **body** (if present) is separated from the description by a blank line
  and explains the *why* of the change, not the *what*
- The **footer** (if present) contains breaking change notices and issue
  references: `Closes #42`, `BREAKING CHANGE: ...`

### 6.2 Types

| Type | Meaning | Example |
|------|---------|---------|
| `feat` | New feature | `feat(ive): add liveness verification pass` |
| `fix` | Bug fix | `fix(codegen): correct LDXR/STXR register allocation` |
| `docs` | Documentation only | `docs(glossary): add ARM64 instruction terms` |
| `test` | Adding or updating tests | `test(ive): add property tests for exclusivity` |
| `refactor` | Code restructuring (no behavior change) | `refactor(scg): extract node visitor trait` |
| `perf` | Performance improvement | `perf(msg): use arena allocation for derivation chains` |
| `chore` | Maintenance, CI, tooling | `chore(ci): update clippy lint configuration` |
| `style` | Formatting only (no logic change) | `style: apply rustfmt` |
| `build` | Build system changes | `build(bare-metal): add linker script` |
| `ci` | CI/CD configuration | `ci: add QEMU aarch64 test job` |

### 6.3 Scopes

Use the crate name as the scope. The valid scopes are:

| Scope | Crate |
|-------|-------|
| `scg` | `vuma-scg` |
| `ive` | `vuma-ive` |
| `vuma` | `vuma` (core) |
| `bd` | `vuma-bd` |
| `cor` | `vuma-cor` |
| `parser` | `vuma-parser` |
| `codegen` | `vuma-codegen` |
| `std` | `vuma-std` |
| `proof` | `vuma-proof` |
| `tests` | `vuma-tests` |
| `package` | `vuma-package` |
| `docs` | Documentation (cross-cutting) |
| `ci` | CI/CD configuration |
| *(none)* | Changes affecting the entire workspace |

### 6.4 Examples

```
feat(ive): add alignment verification pass

Implement the alignment invariant verifier that checks every memory
access targets an address satisfying the alignment requirement of the
access's RepD. The pass integrates into the InvariantAggregator as the
sixth verification pass.

Closes #127
```

```
fix(codegen): correct STXR encoding status register position

The STXR instruction encoding was placing the status register (Rs) in
the wrong bit position (bits 20:16 instead of bits 15:10). This caused
atomic compare-and-swap loops to never observe success.

Fixes #98
```

```
refactor(scg): extract SCGPass trait from transform module

Move the SCGPass trait definition into its own file and add a
PassManager that runs passes in dependency order. No behavior change.
```

### 6.5 Branch Naming

- Feature branches: `feat/<description>` — e.g., `feat/ive-liveness-pass`
- Fix branches: `fix/<description>` — e.g., `fix/codegen-ldxr-register`
- All branches are created from `main` and merged back via PR

### 6.6 PR Titles

PR titles follow the same conventional commit format as commit messages.
Squash-merged PRs produce a single commit whose message is the PR title.

---

*These conventions are enforced by CI and code review. When in doubt, follow
the principle of making the code as easy as possible for the IVE to reason
about — explicit, well-structured, and thoroughly documented.*
