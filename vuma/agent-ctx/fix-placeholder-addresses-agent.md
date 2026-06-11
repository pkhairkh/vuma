# Task: Fix Placeholder Addresses in MSG Incremental Delta Application

## Summary

Fixed all placeholder addresses (`Address::from(0u64)` and `size: 0`) in `msg_incremental.rs` that were used as sentinel values when `compute_scg_delta` couldn't determine a region's real base/size/alloc_point from the SCG snapshot alone.

## Changes Made

### 1. `msg_incremental.rs` — Core fixes

**Renamed `DeltaError::DeallocRegionNotFound` → `DeltaError::AllocationNotFound`**
- Better describes the error: the allocation metadata for a region cannot be found
- Updated `Display` impl message from "dealloc targets missing region" to "allocation not found for region"

**Changed `compute_scg_delta` signature** (line ~875)
- Old: `pub fn compute_scg_delta(old_scg: &SCGSnapshot, new_scg: &SCGSnapshot) -> MSGDelta`
- New: `pub fn compute_scg_delta(old_scg: &SCGSnapshot, new_scg: &SCGSnapshot, msg: &MSG) -> Result<MSGDelta, DeltaError>`
- Now accepts a reference to the MSG to look up real region data
- Returns `Result` so that `AllocationNotFound` errors are propagated instead of silently using sentinels

**Fixed `add_node_to_delta`** (line ~912)
- Added `msg: &MSG` parameter and `Result<(), DeltaError>` return type
- For `SCGNode::Dealloc`: looks up the region in the MSG to get real `base`, `size`, `alloc_point`, and `owner_context`
- Returns `Err(DeltaError::AllocationNotFound(region_id))` if the region isn't in the MSG
- **Before**: Used `Address::from(0u64)`, `size: 0`, and `ProgramPoint::new("", 0, 0)` as sentinels
- **After**: Uses the actual values from the existing MSG region

**Fixed `remove_node_from_delta`** (line ~1037)
- Same pattern: added `msg: &MSG` parameter and `Result<(), DeltaError>` return type
- For undo-dealloc (`SCGNode::Dealloc` removal): looks up the region in the MSG for real `base`, `size`, `alloc_point`, and `owner_context`
- **Before**: Used `Address::from(0u64)`, `size: 0`, and `ProgramPoint::new("", 0, 0)` as sentinels
- **After**: Uses the actual values from the existing MSG region

**Simplified `apply_delta`** (line ~613)
- Removed sentinel detection (`if region.base == Address::from(0u64) && region.size == 0`)
- Now uses incoming region data directly since deltas always carry real values

**Removed `merge_region_modification`** function
- Was only needed to merge sentinel values with existing region data
- Since deltas now carry real values, the merge is no longer needed

### 2. Tests added

**Test 28**: `dealloc_delta_uses_real_address_not_placeholder`
- Verifies that a Dealloc SCG node produces a delta with the real address (0xDEAD_0000), size (0x1000), alloc_point, and owner_context from the MSG — never Address::from(0u64) or size: 0

**Test 29**: `undo_dealloc_delta_uses_real_address`
- Verifies that removing a Dealloc SCG node (undo-dealloc) produces a delta with the real address (0xBEEF_0000) and size from the MSG

**Test 30**: `dealloc_delta_missing_region_returns_allocation_not_found`
- Verifies that if the region doesn't exist in the MSG, `compute_scg_delta` returns `Err(DeltaError::AllocationNotFound(region_id))`

### 3. Test 19 updated
- Updated `compute_scg_delta_additions_and_removals` to pass `&MSG` to the new function signature

### 4. `msg_builder.rs` — No changes needed
- The `size: 0` in `RepD` structs (lines 1017, 1021) are representation descriptor sizes for Cast derivations, not region allocation placeholders
- The `size: 0` in test data (line 1964) is intentional (testing `ZeroSizeAllocation` error)
- These are semantically different from the sentinel addresses/size issue

## Cargo check output
```
Checking vuma-codegen v0.1.0
Checking vuma-parser v0.1.0
Checking vuma-cor v0.1.0
Checking vuma-core v0.1.0
Checking vuma v0.1.0
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.94s
```
