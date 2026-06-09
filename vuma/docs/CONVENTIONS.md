# VUMA Coding Conventions

This document defines the coding standards and conventions for the VUMA project. All contributors must follow these conventions to maintain consistency across the codebase and to support the IVE verification workflow.

---

## 1. Rust Style

### 1.1 Base Standard

All VUMA Rust code follows `rustfmt` with default settings as the baseline formatter. Run `cargo fmt` before every commit. The CI pipeline will reject any PR that does not pass `cargo fmt --check`.

Beyond `rustfmt` defaults, we enforce these additional rules:

- **Maximum line length**: 100 characters for code, 80 characters for doc comments.
- **Imports**: Group imports in the following order, separated by blank lines:
  1. Standard library (`use std::...`)
  2. External crates (`use serde::...`, `use petgraph::...`)
  3. Internal crates (`use vuma_scg::...`, `use vuma_ive::...`)
  4. Current crate (`use crate::...`)
- **Trailing commas**: Always use trailing commas in multi-line constructs (function args, struct fields, enum variants, match arms).
- **Match arms**: Always use `{}` blocks for match arms, even single-expression arms. This makes it easier to add debugging or logging later.

### 1.2 Clippy

All code must pass `cargo clippy` with no warnings. The CI pipeline runs clippy with `-D warnings`, so any clippy warning is treated as an error. If a clippy lint is genuinely inappropriate for a specific case, add a targeted `#[allow(clippy::lint_name)]` with a comment explaining why.

---

## 2. Naming Conventions

### 2.1 General Rules

| Item | Convention | Example |
|------|-----------|---------|
| Crates | `snake_case` | `vuma_scg`, `vuma_ive`, `vuma_codegen` |
| Modules | `snake_case` | `memory_model`, `graph_transform` |
| Types (struct, enum, trait) | `CamelCase` | `SemanticComputationGraph`, `BehavioralDescriptor` |
| Enum variants | `CamelCase` | `LivenessViolation`, `ExclusivityViolation` |
| Functions / methods | `snake_case` | `verify_liveness`, `infer_capabilities` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_VERIFICATION_DEPTH`, `DEFAULT_REGION_SIZE` |
| Static variables | `SCREAMING_SNAKE_CASE` | `GLOBAL_IVE_CONFIG` |
| Type parameters | Short `CamelCase` (single letter for common, descriptive for domain) | `T`, `N`, `NodeKind`, `RepDescriptor` |
| Lifetime parameters | Short lowercase | `'a`, `'ctx`, `'graph` |
| Macros | `snake_case!` | `scg_node!`, `define_bd!` |

### 2.2 VUMA-Specific Naming

- **SCG node types**: Named as `{Kind}Node` — e.g., `AllocationNode`, `FunctionCallNode`, `EffectNode`.
- **BD components**: Named as `{DescriptorKind}Descriptor` — e.g., `RepresentationDescriptor`, `CapabilityDescriptor`, `RelationalDescriptor`. In context, the abbreviations `RepD`, `CapD`, `RelD` are acceptable in doc comments and local variables but **never** in public API names.
- **Verification result types**: Named as `{Property}Result` — e.g., `LivenessResult`, `ExclusivityResult`.
- **Error types**: Named as `{Domain}Error` — e.g., `ScgError`, `IveError`, `CodegenError`.
- **ARM64 instruction types**: Named exactly as the ARM64 mnemonic in `CamelCase` — e.g., `Ldxr`, `Stxr`, `Dmb`, `Dsb`, `Isb`.

---

## 3. Module Organization

### 3.1 File Structure

- **One struct per file** for large types (anything over ~200 lines of implementation). The file name matches the struct name in `snake_case`: `SemanticComputationGraph` lives in `semantic_computation_graph.rs`.
- **Related small types** can share a file. For example, `RepD`, `CapD`, and `RelD` descriptor types that are tightly coupled and under 100 lines each may live in `descriptors.rs`.
- **Module re-exports**: Every module has a `mod.rs` (or a module file) that re-exports the public API. Internal types should not be exposed at the crate root.
- **Workspace layout**: Each workspace member (`src/scg`, `src/ive`, `src/codegen`, etc.) is a self-contained crate with its own `Cargo.toml`. Cross-crate dependencies go through the public API only — no `#[path = "..."]` hacks.

### 3.2 Crate Organization

```
vuma/
├── src/
│   ├── scg/          # Semantic Computation Graph
│   ├── ive/          # Inference and Verification Engine
│   ├── vuma/         # VUMA memory model (Address, Region, Access)
│   ├── bd/           # Behavioral Descriptors (RepD, CapD, RelD)
│   ├── cor/          # Continuous Optimization Runtime
│   ├── parser/       # SCG parser and projection parser
│   ├── codegen/      # ARM64 code generation
│   ├── pi5/          # Pi 5 platform abstraction
│   ├── std/          # VUMA standard library
│   ├── proof/        # Proof infrastructure and formal methods
│   ├── projection/   # Projection system (textual, visual, conversational)
│   └── tests/        # Integration tests
├── docs/             # Documentation (this directory)
└── Cargo.toml        # Workspace root
```

---

## 4. Error Handling

### 4.1 Library Crates (vuma-scg, vuma-ive, vuma-bd, vuma-codegen, etc.)

Use **`thiserror`** for all error types in library crates. This ensures that error types are concrete, implement `std::error::Error`, and have useful `Display` implementations.

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
}
```

### 4.2 Application Crates (vuma-cli, integration tests, examples)

Use **`anyhow`** for application-level error handling. This allows flexible error chaining and context attachment without defining exhaustive error enums.

```rust
use anyhow::{Context, Result};

fn run_verification(scg: &SemanticComputationGraph) -> Result<()> {
    let msg = scg.build_memory_state_graph()
        .context("failed to build Memory State Graph")?;
    ive::verify_liveness(&msg)
        .context("liveness verification failed")?;
    Ok(())
}
```

### 4.3 No Panics in Library Code

Library crates must **never panic** in production code. Use `Result` for all fallible operations. The only acceptable uses of `panic!`, `unwrap()`, or `expect()` are:

1. In test code (`#[cfg(test)]` modules).
2. For true programming errors that indicate a bug (e.g., `assert_eq!` on an invariant that must hold). In this case, include a message explaining the invariant.
3. In `unreachable!()` branches after exhaustive matches.

---

## 5. Documentation

### 5.1 Doc Comments Are Mandatory

**All public items** (functions, types, traits, modules, constants) must have doc comments (`///` or `//!`). Doc comments must include:

1. **A summary line** (imperative mood, no period): `/// Verify liveness of all memory accesses`
2. **A detailed description** (one or more paragraphs explaining the what, why, and how)
3. **At least one example** for functions and methods showing typical usage
4. **Panics** section if the function can panic (explain under what conditions)
5. **Errors** section if the function returns `Result` (describe what errors can be returned)
6. **Safety** section for any `// VUMA-VERIFIED` code (explain what the IVE verified)

Example:

```rust
/// Verify liveness of all memory accesses in the Memory State Graph
///
/// Traverses every access node in the MSG and checks that the target region
/// is allocated at the program point where the access occurs. This is the
/// first of the five VUMA global invariants (see GLOSSARY.md for details).
///
/// # Examples
///
/// ```
/// use vuma_ive::verify_liveness;
/// use vuma_vuma::MemoryStateGraph;
///
/// let msg = MemoryStateGraph::from_scg(&scg);
/// let result = verify_liveness(&msg);
/// assert!(result.is_ok(), "all accesses target live memory");
/// ```
///
/// # Errors
///
/// Returns `IveError::LivenessViolation` if any access targets a freed
/// or unallocated region, with the offending address and execution path.
pub fn verify_liveness(msg: &MemoryStateGraph) -> Result<LivenessReport, IveError> {
    // ...
}
```

### 5.2 Module-Level Documentation

Every crate must have a `//!` module-level doc comment in its `lib.rs` (or `main.rs`) that explains:

1. What the crate does
2. How it fits into the VUMA architecture
3. Key types and their relationships
4. Links to related crates

### 5.3 Internal Documentation

Private functions and types should also have doc comments when their purpose is not immediately obvious from the name. A good rule of thumb: if you would need to read the function body to understand what it does, it needs a doc comment.

---

## 6. Testing

### 6.1 Unit Tests

Unit tests live in the same file as the code they test, inside a `#[cfg(test)] mod tests` block at the bottom of the file. Unit tests should be focused, fast, and deterministic. Each test function name should describe the specific scenario being tested:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_liveness_catches_use_after_free() {
        // ...
    }

    #[test]
    fn verify_liveness_accepts_valid_access() {
        // ...
    }

    #[test]
    fn verify_liveness_flags_uninitialized_read() {
        // ...
    }
}
```

### 6.2 Integration Tests

Integration tests live in the `src/tests/` workspace crate (for cross-crate integration tests) or in the `tests/` directory of individual crates. Integration tests should exercise complete workflows: building an SCG, running IVE verification, generating ARM64 code, and verifying the output.

### 6.3 Property-Based Testing

For verification-related code, prefer property-based tests using the `proptest` crate. This is especially important for the IVE, where edge cases in pointer arithmetic and graph traversal are common.

### 6.4 Test Naming Convention

Test functions use `snake_case` and follow the pattern: `{method}_{scenario}_{expected_outcome}`.

- `verify_liveness_valid_access_returns_ok`
- `verify_liveness_freed_region_returns_violation`
- `codegen_ldxr_stxr_produces_valid_atomic`

---

## 7. Unsafe Code Policy

### 7.1 No Bare `unsafe` in VUMA Stdlib

The VUMA standard library (`src/std/`) must **not** contain any bare `unsafe` blocks. All raw memory access in the stdlib must be annotated with one of two markers:

- **`// VUMA-VERIFIED`**: The IVE has formally verified that this access is safe. The comment must include a brief description of what was verified:
  ```rust
  // VUMA-VERIFIED: IVE proves this dereference targets a live, aligned region
  let value = *ptr;
  ```

- **`// IVE-TODO`**: IVE verification has not yet been implemented for this access. This is a temporary state that must be resolved before the next release:
  ```rust
  // IVE-TODO: verify origin and liveness for this pointer arithmetic
  let offset = base.add(64);
  ```

### 7.2 Unsafe in Other Crates

The `vuma-codegen` and `vuma-pi5` crates may contain `unsafe` blocks for FFI and hardware access, but these must also be annotated with `// VUMA-VERIFIED` or `// IVE-TODO`. Additionally, every `unsafe` block must have a `// SAFETY:` comment explaining why the operation is sound, following the Rust standard library convention.

### 7.3 Auditing

The CI pipeline includes a step that scans for `unsafe` blocks without the required annotations. Any PR that introduces an un-annotated `unsafe` block will be rejected.

---

## 8. Comment Conventions

### 8.1 Verification Annotations

| Annotation | Meaning | When to Use |
|-----------|---------|------------|
| `// VUMA-VERIFIED: <what>` | IVE has proven safety | After IVE verification is complete |
| `// IVE-TODO: <what>` | IVE verification pending | For any raw memory access not yet verified |
| `// SAFETY: <why>` | Manual safety argument | For `unsafe` blocks in codegen/pi5 crates |

### 8.2 Code Comments

- Use `//` for inline comments and `///` for doc comments.
- Prefer expressive code over comments. If you feel the need to explain *what* the code does, consider renaming or restructuring first.
- Comments should explain *why*, not *what*: `// We check exclusivity before liveness because exclusivity violations` \
  `// are cheaper to detect and short-circuit the more expensive liveness analysis.`
- Use `// TODO:` for general (non-verification) tasks, with a brief description and ideally a tracking issue number: `// TODO(#42): handle recursive data structures in MSG construction`

### 8.3 Section Comments

For long functions or modules, use section comments to delineate logical groups:

```rust
// --- Allocation Tracking ---

fn track_allocation(&mut self, region: Region) { ... }

fn track_deallocation(&mut self, region_id: RegionId) { ... }

// --- Pointer Derivation ---

fn track_derivation(&mut self, from: Address, to: Address) { ... }
```

---

## 9. Git Conventions

### 9.1 Conventional Commits

All commit messages must follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

**Types:**

| Type | Meaning | Example |
|------|---------|---------|
| `feat` | New feature | `feat(ive): add liveness verification pass` |
| `fix` | Bug fix | `fix(codegen): correct LDXR/STXR register allocation` |
| `docs` | Documentation | `docs(glossary): add ARM64 instruction terms` |
| `test` | Tests | `test(ive): add property tests for exclusivity` |
| `refactor` | Code restructuring | `refactor(scg): extract node visitor trait` |
| `perf` | Performance improvement | `perf(msg): use arena allocation for derivation chains` |
| `chore` | Maintenance | `chore(ci): update clippy lint configuration` |
| `style` | Formatting only | `style: apply rustfmt` |

**Scopes:** Use the crate name as the scope: `scg`, `ive`, `bd`, `vuma`, `cor`, `codegen`, `pi5`, `parser`, `proof`, `std`, `projection`.

### 9.2 Branch Naming

- Feature branches: `feat/<description>` — e.g., `feat/ive-liveness-pass`
- Fix branches: `fix/<description>` — e.g., `fix/codegen-ldxr-register`
- All branches are created from `main` and merged back via PR.

### 9.3 PR Titles

PR titles follow the same conventional commit format. The PR body must include:
1. A summary of changes
2. Links to any related issues
3. Verification status (which IVE invariants are affected, any new IVE-TODOs introduced)
4. Test plan (what tests were added/modified)

---

## 10. Dependency Policy

### 10.1 Allowed Dependencies

The VUMA workspace uses a curated set of dependencies defined in the root `Cargo.toml`. Adding a new dependency requires justification in the PR:

- **Why is this dependency needed?** (What does it do that we cannot reasonably implement ourselves?)
- **What is its license?** (Must be MIT, Apache-2.0, or BSD-2/3-Clause)
- **What is its maintenance status?** (Actively maintained, well-adopted)

### 10.2 Current Dependencies

| Crate | Purpose | Notes |
|-------|---------|-------|
| `serde` / `serde_json` | Serialization of SCG, BD, MSG | Required for persistence and IPC |
| `petgraph` | Graph data structures | Used for SCG and MSG representation |
| `thiserror` | Library error types | Mandatory for all library crates |
| `anyhow` | Application error types | Mandatory for application crates |
| `clap` | CLI argument parsing | For vuma-cli |
| `log` / `env_logger` | Logging | Structured logging throughout |
| `colored` | Terminal output formatting | For verification reports |
| `chrono` | Timestamps | For profiling and verification timing |
| `indexmap` | Ordered maps | For deterministic iteration in SCG |
| `smallvec` | Small vector optimization | For node annotations |
| `hashbrown` | High-performance hash maps | For IVE internal data structures |

---

*These conventions are enforced by CI. When in doubt, follow the principle of making the code as easy as possible for the IVE to reason about — explicit, well-structured, and thoroughly documented.*
