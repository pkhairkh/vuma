# VUMA Project Audit Report

**Generated:** 2026-03-05
**Auditor:** Task 5-10 (Automated Audit)

---

## 1. Source Code Metrics

| Metric | Value |
|---|---|
| Total `.rs` files | 118 |
| Total Rust lines of code | 116,929 |
| Total `.md` documentation files | 26 |
| Total documentation lines | 17,412 |
| Total `.vuma` example files | 10 |
| Total files in project (excl. target/, .git/) | 175 |

## 2. Workspace Structure

| Metric | Value |
|---|---|
| Workspace crates | 12 |
| `pub mod` declarations in `lib.rs` files | 102 |

### Crates

| Crate | Path | `pub mod` count |
|---|---|---|
| `vuma` (root) | `src/` | 1 |
| `vuma-scg` | `src/scg/` | 10 |
| `vuma-ive` | `src/ive/` | 12 |
| `vuma-core` | `src/vuma/` | 17 |
| `vuma-bd` | `src/bd/` | 11 |
| `vuma-cor` | `src/cor/` | 7 |
| `vuma-projection` | `src/projection/` | 5 |
| `vuma-parser` | `src/parser/` | 5 |
| `vuma-codegen` | `src/codegen/` | 5 |
| `vuma-pi5` | `src/pi5/` | 7 |
| `vuma-std` | `src/std/` | 5 |
| `vuma-proof` | `src/proof/` | 10 |
| `vuma-tests` | `src/tests/` | 7 |

## 3. Test Coverage

| Metric | Value |
|---|---|
| Total `#[test]` functions | 1,772 |

## 4. Example Programs (`.vuma`)

| File | Description |
|---|---|
| `examples/hello_memory.vuma` | Basic allocate/write/read/free |
| `examples/doubly_linked_list.vuma` | Doubly-linked list with sentinel |
| `examples/gpio_blink.vuma` | Pi 5 GPIO hardware access |
| `examples/arena_allocator.vuma` | Arena allocator with derivations |
| `examples/thread_pool.vuma` | Thread pool |
| `examples/channel_demo.vuma` | Channel concurrency demo |
| `examples/memory_arena.vuma` | Memory arena |
| `examples/pi5_sensor.vuma` | Pi 5 sensor reading |
| `examples/lock_free_queue.vuma` | Lock-free SPSC queue with atomics |
| `examples/sorted_map.vuma` | Sorted map |

## 5. Compilation Status (`cargo check`)

| Crate | Status | Notes |
|---|---|---|
| `vuma-scg` | PASS | 2 warnings (dead code in serialize.rs) |
| `vuma-ive` | PASS | 3 warnings |
| `vuma-bd` | PASS | Clean |
| `vuma-cor` | PASS | Clean |
| `vuma-projection` | PASS | 1 warning |
| `vuma-codegen` | PASS | 5 warnings (4 fixable via `cargo fix`) |
| `vuma-std` | PASS | 11 warnings |
| `vuma-proof` | PASS | 3 warnings (2 fixable via `cargo fix`) |
| `vuma-pi5` | PASS | 6 warnings (mutable static references) |
| `vuma-parser` | FAIL | 7 errors (E0004: mismatched types) |
| `vuma-core` | FAIL | Blocked by vuma-parser failure |
| `vuma-tests` | FAIL | Blocked by vuma-parser failure |

### Key Findings

- **9 of 12 crates compile successfully** via `cargo check`.
- **`vuma-parser` has 7 type mismatch errors** (E0004) that block the 3 downstream crates (`vuma-core`, `vuma-tests`, and the root `vuma` crate).
- `vuma-scg` and `vuma-bd` both pass `cargo check` cleanly.
- The `vuma-core` crate has `access_analysis` module commented out (`// pub mod access_analysis;`) with a note "compile errors from other agent", indicating a known issue.

## 6. Documentation Inventory

| Directory | Files | Purpose |
|---|---|---|
| `docs/` | 8 | Top-level docs (architecture, roadmap, contributing, conventions, glossary, language-reference, worklog) |
| `docs/specs/` | 14 | Formal specifications (SCG, BD, IVE, MSG, proof, codegen, security, decidability, etc.) |
| Root | 2 | MANIFEST.md, WORKLOG.md |
| `examples/` | 1 | examples/README.md |
| **Total** | **26** | |

## 7. Summary

The VUMA project is a substantial codebase with ~117K lines of Rust across 12 workspace crates, 1,772 test functions, 102 public modules, and 26 documentation files. The architecture follows a layered design (SCG -> BD -> IVE -> VUMA -> Codegen/COR -> Pi5) with formal specifications for each component.

**Primary risk:** The `vuma-parser` crate has 7 compilation errors that cascade to block 3 other crates. Fixing the parser type mismatches should be the highest priority to restore full workspace compilation.

---

## Worklog

### 2026-03-05 — Task 5-10: Project Audit

**Actions performed:**
1. Enumerated all `.rs` files (118) and computed total Rust LOC (116,929)
2. Enumerated all `.md` documentation files (26) and computed total documentation lines (17,412)
3. Enumerated `.vuma` example files (10)
4. Parsed `Cargo.toml` workspace members (12 crates)
5. Counted `#[test]` annotations across all `.rs` files (1,772 test functions)
6. Counted `pub mod` declarations in all `lib.rs` files (102 modules)
7. Counted total project files excluding target/ and .git/ (175)
8. Ran `cargo check` on each crate individually:
   - PASS: vuma-scg, vuma-ive, vuma-bd, vuma-cor, vuma-projection, vuma-codegen, vuma-std, vuma-proof, vuma-pi5
   - FAIL: vuma-parser (7x E0004), vuma-core (blocked), vuma-tests (blocked)
9. Wrote this audit report to `docs/AUDIT.md`

**No source code modifications were made.** This was a read-only audit.
