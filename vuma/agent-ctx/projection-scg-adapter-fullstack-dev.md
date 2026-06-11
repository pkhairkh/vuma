# Task: Update projection conversational.rs module to support real vuma-scg types and add bidirectional verification tests

## Task ID: projection-scg-adapter

## Summary

Updated the vuma-projection crate to support real `vuma-scg` types with bidirectional conversion and verification tests. All 94 unit tests + 2 doc tests pass.

## Changes Made

### 1. `src/projection/Cargo.toml`
- Already had `vuma-scg = { path = "../scg" }` dependency (no change needed)

### 2. NEW: `src/projection/src/scg_adapter.rs`
- Created bidirectional conversion module between `vuma_scg` types and projection placeholder types
- **Forward conversion (vuma-scg → projection):**
  - `node_type_to_kind()` — Maps NodeType → NodeKind with control-flow refinement
  - `control_kind_to_node_kind()` — Refines Control nodes to Function or Merge
  - `scg_edge_kind_to_proj()` — Maps EdgeKind with Call/Dispatch/Return handling
  - `derive_label()` — Derives human-readable labels from NodeData payloads
  - `refined_node_kind()` — Determines NodeKind with Control refinement
  - `from_node_data()` — Converts NodeData → SCGNode
  - `from_edge_data()` — Converts EdgeData → SCGEdge
  - `from_scg_region()` — Converts SCGRegion → SCGRegion
  - `from_scg()` — Full SCG conversion with region membership population
- **Reverse conversion (projection → vuma-scg):**
  - `node_kind_to_type()` — Maps NodeKind → NodeType
  - `node_kind_to_control_kind()` — Refines Function/Merge to ControlKind
  - `proj_edge_kind_to_scg()` — Maps projection EdgeKind → vuma-scg EdgeKind
  - `node_kind_to_payload()` — Creates default payloads for each NodeKind
  - `to_scg()` — Full reverse SCG conversion
- 3 unit tests for adapter functions

### 3. UPDATED: `src/projection/src/conversational.rs`
- Added `use vuma_scg;` at the top
- Added `ConversationalSession` struct — wraps a projection SCG with a `ConversationalProjection` engine
  - `new()`, `with_verbosity()`, `render()`, `explain_node()`, `explain_region()`, `query()`, `scg()`
- Added `session_from_scg()` function — creates a ConversationalSession from a real vuma-scg SCG
- Added 2 new tests:
  - `test_session_from_real_scg` — creates a real SCG with allocation/computation nodes, creates a session, verifies render/query/explain_node work
  - `test_conversational_roundtrip` — creates a real SCG, converts to projection, gets conversational output, converts back, verifies structural equivalence (node count, edge count, node types)

### 4. NEW: `src/projection/src/verification.rs`
- Created bidirectional verification tests module with 3 tests:
  - `test_full_scg_roundtrip` — Full SCG with alloc/compute/dealloc nodes, DataFlow/ControlFlow/Derivation edges, and a region; verifies node count, edge count, region count, node types, and region membership are preserved
  - `test_all_node_types_roundtrip` — Tests all 6 NodeType variants (Computation, Allocation, Deallocation, Access, Effect, Control); verifies each type is preserved through roundtrip
  - `test_all_edge_types_roundtrip` — Tests DataFlow, ControlFlow, Derivation, Annotation edge kinds; verifies each is preserved through roundtrip

### 5. UPDATED: `src/projection/src/lib.rs`
- Added `pub mod verification;` module declaration
- Added `ConversationalSession` and `session_from_scg` to re-exports

## Test Results

```
94 unit tests passed, 0 failed
2 doc tests passed, 0 failed
```

All existing tests continue to pass; no regressions.
