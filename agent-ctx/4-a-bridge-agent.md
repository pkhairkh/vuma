# Task 4-a: Add vuma-scg Dependency and Create SCG Bridge

## Agent: 4-a
## Date: 2026-03-06
## Status: ✅ Complete

### Summary
Added `vuma-scg` as a dependency to `vuma-cor` and created a bridge from `vuma_scg::SCG` to `vuma_cor::types::SCG`. The bridge implements `From<vuma_scg::SCG> for vuma_cor::types::SCG`, mapping the fine-grained SCG node types to the coarser COR node kinds and assigning edge weights based on edge kind. A convenience method `CORuntime::from_vuma_scg()` was added so consumers can construct a runtime directly from a `vuma_scg::SCG` without knowing about the bridge module.

### Files Created/Modified
- `src/cor/Cargo.toml` — Added `vuma-scg = { path = "../scg" }` dependency
- `src/cor/src/bridge.rs` — Created bridge module with `From<vuma_scg::SCG> for SCG` impl, `map_node_type()`, `edge_weight()`, and 5 unit tests
- `src/cor/src/lib.rs` — Added `pub mod bridge;` declaration
- `src/cor/src/runtime.rs` — Added `CORuntime::from_vuma_scg()` convenience method

### Build & Test Results
- `cargo check -p vuma-cor` — PASS
- `cargo test -p vuma-cor --lib bridge` — 5/5 passed
- `cargo test -p vuma-cor --lib` — 72/72 passed
