# Task 4: COR Bridge Enricher

## Task
Enrich COR bridge and rewrite node_to_statements() so the COR runtime can produce real control flow instead of trivial Return(Int(N)) stubs.

## Work Completed

### Files Modified
1. `src/cor/src/types.rs` — Added 6 new NodeKind variants (LoopHeader, LoopExit, Join, FunctionEntry, FunctionReturn, Jump) and `control_label: Option<String>` field to SCGNode
2. `src/cor/src/bridge.rs` — Rewrote `map_node_type()` to inspect Control payload for fine-grained mapping; added `extract_control_label()` helper; updated `From` impl; updated all tests
3. `src/cor/src/runtime.rs` — Rewrote `node_to_statements()` to produce real codegen IR (loops, branches, allocations, loads, computations); added BinOpKind import
4. `src/cor/src/optimization.rs` — Updated LoopOptimization to also match NodeKind::LoopHeader for unrolling

### Key Changes
- Bridge now preserves fine-grained ControlKind information (Branch, LoopHeader, LoopExit, Join, FunctionEntry, FunctionReturn, Jump) instead of collapsing all Control nodes to Entry
- COR runtime now generates real codegen IR statements: ComputationNode with BinOp, AllocationNode::Stack + AccessNode::Load for Memory, ControlNode::Loop with unrolled body, ControlNode::If for Branch, ControlNode::Break for Jump
- Optimization engine now correctly identifies LoopHeader nodes for unrolling

### Test Results
- 78 vuma-cor tests pass
- 5 e2e_cor integration tests pass
- 12 vuma main crate tests pass
- No new compilation errors
