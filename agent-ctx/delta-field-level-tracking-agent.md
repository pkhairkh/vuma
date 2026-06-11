# Task: Enhance COR Delta with Field-Level Change Tracking

## Summary

Enhanced the COR `Delta` struct to track field-level changes for incremental recompilation instead of just node/edge IDs. All changes maintain backward compatibility with existing construction sites.

## Changes Made

### 1. `src/cor/src/types.rs` — Core Delta enhancement

**New types added:**
- `FieldChange` — records field name, old value, and new value for a single field change (derives `Serialize, Deserialize, PartialEq, Eq`)
- `NodeDelta` — rich information about an added node (ID + optional kind/code_size), with `From<NodeId>` for easy construction
- `EdgeDelta` — rich information about an added edge (ID + optional source/target), with `From<EdgeId>` for easy construction
- `NodeModification` — field-level changes for a modified node (node_id + Vec<FieldChange>)
- `EdgeModification` — field-level changes for a modified edge (edge_id + Vec<FieldChange>)
- `RegionDelta` — field-level changes for a region (region_id + Vec<FieldChange>)

**Delta struct changes:**
- Added `Serialize, Deserialize` derives
- Kept `added_nodes: Vec<NodeId>` and `added_edges: Vec<EdgeId>` for backward compat
- Added `modified_nodes: Vec<NodeModification>` — tracks which node fields changed
- Added `modified_edges: Vec<EdgeModification>` — tracks which edge fields changed
- Added `region_changes: Vec<RegionDelta>` — tracks region-level changes
- Updated `empty()` and `is_empty()` to include new fields
- Added `total_field_changes()` utility method
- Added `Default` impl (delegates to `empty()`)

**New functions:**
- `diff_nodes(old: &SCGNode, new: &SCGNode) -> Vec<FieldChange>` — compares all fields of two nodes (kind, edges, code_size, is_inlined, is_outlined, unroll_factor, is_vectorized, alignment, has_prefetch, control_label)
- `diff_edges(old: &SCGEdge, new: &SCGEdge) -> Vec<FieldChange>` — compares source, target, weight fields

**`NodeKind` now derives `Serialize, Deserialize`** (needed for `NodeDelta`)

**12 tests added** including the two required:
- `test_node_modification_detects_field_changes` — verifies diff_nodes detects 3 field changes (is_inlined, unroll_factor, code_size)
- `test_delta_field_level_diff` — end-to-end test: create nodes, diff, construct Delta, verify
- Plus 10 additional tests covering no-change cases, edge diffs, multi-modification deltas, NodeDelta/EdgeDelta construction, FieldChange equality, Default impl, control_label changes

### 2. `src/cor/src/runtime.rs` — compile_incremental handles modifications

- `compile_incremental` now processes `modified_nodes` and `modified_edges`
- When a modified node's region is already compiled, it invalidates and recompiles it
- When a modified edge's connected regions are compiled, they are invalidated and recompiled
- Enhanced logging includes modification counts and total field changes
- All Delta construction sites updated to use `..Delta::empty()` syntax

### 3. `src/pipeline.rs` — Updated Delta construction

- Changed to use `..vuma_cor::types::Delta::empty()` for backward compat

### 4. `src/tests/src/e2e_cor.rs` — Updated 3 Delta construction sites

- All 3 sites changed to use `..Delta::empty()`

### 5. Minor pre-existing test fixes

- `src/cor/src/profile.rs:1040` — added `mut` to guard for `record_access` call
- `src/cor/src/speculative.rs:1279` — removed unnecessary `mut` from inliner

## Cargo Check Output

```
    Checking vuma-cor v0.1.0 (/home/z/my-project/vuma/src/cor)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s
```

Also verified: `cargo check -p vuma-tests` passes (full dependency chain compiles).

## Test Results

All 12 new types tests pass + doc test passes. 113/114 total tests pass (1 pre-existing failure in `test_compiled_region_stores_code` unrelated to this change).
