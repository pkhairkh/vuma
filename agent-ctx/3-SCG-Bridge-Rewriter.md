# Task 3: SCG Bridge Rewriter

## Task
Rewrite `bridge_scg_to_codegen()` in `/home/z/my-project/vuma/src/pipeline.rs` to reconstruct real control flow (if/else, loops, function boundaries, break/continue) from the vuma_scg::SCG instead of just dropping all Control nodes.

## What Was Done

### Core Changes (pipeline.rs)
- Replaced the flat topological-walk bridge (~100 lines) with a structured control flow reconstruction (~780 lines)
- Added `EdgeIndex` struct for efficient edge lookup by node ID and edge kind
- Added imports: `HashMap`, `HashSet`, `VecDeque`, `EdgeData`, `ControlKind`, `AccessMode`, `NodePayload`

### New Functions
1. `EdgeIndex::build()` — builds outgoing/incoming edge indices from SCG
2. `EdgeIndex::outgoing_cf()` — outgoing ControlFlow edges
3. `EdgeIndex::incoming_df()` — incoming DataFlow edges
4. `EdgeIndex::outgoing_df()` — outgoing DataFlow edges
5. `node_var()` — variable name generation for nodes
6. `resolve_df_input()` — resolve DataFlow inputs for operand naming
7. `resolve_branch_cond()` — resolve Branch condition from DataFlow
8. `find_function_return()` — BFS from FunctionEntry to FunctionReturn
9. `find_reachable_joins()` — BFS to find Join convergence points
10. `find_join_for_branch()` — find Join where Branch arms converge
11. `resolve_branch()` — resolve then/else targets from labeled CF edges
12. `resolve_loop()` — resolve body/exit from LoopHeader's CF edges
13. `walk_control_flow()` — recursive walk producing ScgStatements with control flow
14. `convert_node_to_statement()` — converts all node types to ScgStatements
15. `extract_function_params()` — extract params from FunctionEntry DataFlow edges
16. `parse_scg_type()` — parse type strings to ScgType
17. `find_entry_points()` — find entry points for no-FunctionEntry case

### Control Flow Reconstruction
- **Branch+Join** → `ControlNode::If { cond, then_body, else_body }`
- **LoopHeader+LoopExit** → `ControlNode::Loop { body }`
- **Jump("break")** → `ControlNode::Break`
- **Jump("continue")** → `ControlNode::Continue`
- **FunctionReturn** → `ScgStatement::Return`

### Access Write Mode (NEW)
- `AccessMode::Write` and `AccessMode::ReadWrite` now generate `AccessNode::Store`
- Previously all Access nodes were treated as Load regardless of mode

### Function Boundary Detection
- `FunctionEntry` nodes define function starts
- Parameters extracted from DataFlow edges
- `FunctionReturn` found via BFS
- Body nodes walked via control flow
- Remaining nodes collected separately

## Build & Test Results
- `cargo check -p vuma` — 0 errors
- `cargo check -p vuma-core` — 0 errors
- `cargo test -p vuma --lib` — 12/12 tests pass
