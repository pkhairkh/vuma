# Task W6-T3: Loop Detection for VUMA SCG Module

## Summary
Added loop detection to the VUMA SCG module with CFG-only dominator computation, natural loop detection, loop nesting tree construction, infinite loop detection, and loop-invariant node identification.

## Key Changes

### `/home/z/my-project/vuma/src/scg/src/loop_detection.rs`
Complete rewrite with:
- **`NaturalLoop` struct**: header, backedge_source, body (HashSet<NodeId>), exits (HashSet<NodeId>), depth
- **`LoopNestingTree` struct**: loops (Vec<NaturalLoop>), parent (HashMap<usize, Option<usize>>)
- **`LoopDetector`** with 4 public methods:
  - `detect_natural_loops(scg: &SCG) -> Vec<NaturalLoop>`
  - `detect_loop_nesting(scg: &SCG) -> LoopNestingTree`
  - `detect_infinite_loops(scg: &SCG) -> Vec<NodeId>`
  - `loop_invariant_nodes(loop: &NaturalLoop, scg: &SCG) -> Vec<NodeId>`
- **Internal `CfgDomTree`**: CFG-only dominator tree using Cooper-Harvey-Kennedy algorithm
- **13 tests** covering all functionality

### `/home/z/my-project/vuma/src/scg/src/lib.rs`
- Added `pub mod loop_detection;`
- Added re-exports: `LoopDetector`, `LoopNestingTree`, `NaturalLoop`

## Critical Design Decision
The original implementation used `compute_dominators` from the `dominance` module, which operates on ALL edges (including DataFlow). This caused incorrect back-edge detection because DataFlow edges create alternative paths that bypass loop headers, preventing the dominator tree from recognizing that the header dominates loop body nodes.

The fix was to implement a dedicated `compute_cfg_dominators` that only considers ControlFlow edges, using the iterative Cooper-Harvey-Kennedy algorithm with reverse postorder and RPO-based intersect.

## Verification
- 173 tests pass (0 failures), including 13 loop_detection tests
