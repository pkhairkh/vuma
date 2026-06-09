# VUMA Project Worklog

## Task W1-08: SCG Crate Scaffold
**Date:** 2026-03-05
**Agent:** W1-08
**Status:** ✅ Complete

### Summary
Created the `vuma-scg` Rust crate — the Semantic Computation Graph module — with full node/edge types, directed graph structure backed by petgraph, memory regions, query engine, and validation.

### Files Created
| File | Description |
|------|-------------|
| `src/scg/Cargo.toml` | Crate manifest (deps: serde, petgraph, indexmap, smallvec, hashbrown with serde feature) |
| `src/scg/src/lib.rs` | Root module with re-exports, crate-level docs, integration test |
| `src/scg/src/node.rs` | `NodeId` (newtype u64), `NodeType` enum (8 variants), `NodeData`, `NodePayload` enum, per-variant structs (`AllocationNode`, `DeallocationNode`, `AccessNode`, `CastNode`, `EffectNode`, `ControlNode`, `PhantomNode`), `BDReference`, `ProgramPoint`, `AccessMode`, `ControlKind` |
| `src/scg/src/edge.rs` | `EdgeId` (newtype u64), `EdgeKind` enum (DataFlow, ControlFlow, Derivation, Annotation), `EdgeData` with builder methods |
| `src/scg/src/graph.rs` | `SCG` struct wrapping `DiGraph<NodeData, EdgeData>`, bidirectional NodeId/EdgeId ↔ petgraph index mappings, `SCGError` enum, `ValidationResult`; methods: `add_node`, `add_edge`, `remove_node`, `remove_edge`, `get_node`, `get_edge`, `successors`, `predecessors`, `find_path`, `topological_sort`, `validate`, `merge` |
| `src/scg/src/region.rs` | `RegionId` (newtype u64), `DeploymentTarget` enum (Heap, Stack, Gpu, Shared, Persisted, Custom), `SCGRegion` with node set, scope_level, security_boundary |
| `src/scg/src/query.rs` | `SCGQuery` enum (8 variants), `QueryResult`, `DerivationChain`, `execute()` dispatcher, `find_access_nodes_to_region()`, `find_derivation_chains()`, DFS-based path finding, data-flow reachability, leaked allocation detection |

### Key Design Decisions
1. **External ID ↔ petgraph index bidirectional mapping** — `NodeId`/`EdgeId` are stable external identifiers; petgraph's internal `NodeIndex`/`EdgeIndex` may shift on removal, so mappings are rebuilt after node removal.
2. **hashbrown with serde feature** — Required for `SCGRegion.nodes: hashbrown::HashSet<NodeId>` to derive `Serialize`/`Deserialize`. Version 0.14 chosen for compatibility.
3. **Borrow-checker-friendly getters** — `get_node_mut` and `get_edge_mut` use two-step lookup (copy index, then get mutable ref) to avoid simultaneous immutable+mutable borrows.
4. **Edge endpoints validated before allocation** — `add_edge` copies `source_idx`/`target_idx` before calling `alloc_edge_id()` to avoid borrow conflicts.
5. **`petgraph::visit::EdgeRef` trait import** — Required for `e.id()` on `EdgeReference` in `remove_node`.

### Test Results
```
35 tests passed, 0 failed, 1 doc-test passed
- node: NodeId creation/display, NodeType display, AllocationNode, AccessNode modes, CastNode
- edge: EdgeId creation/display, EdgeKind display, EdgeData new/with_label/builder
- graph: add/get node, remove node, add/get edge, invalid endpoints, successors/predecessors,
         find_path, topological_sort (acyclic + cyclic), validate (clean + missing dealloc),
         merge, regions
- region: RegionId, DeploymentTarget display, add/remove nodes, security_boundary
- query: NodesByType, AccessNodesToRegion, LeakedAllocations, DerivationChains, EdgesByKind,
         NodesByRegion
- integration: build→validate→query pipeline
- doc-test: lib.rs quick-start example
```

### Next Actions
- Implement `Serialize`/`Deserialize` for `SCG` (graph serialization/deserialization)
- Add graph visualization (DOT format export)
- Add incremental graph update APIs for compiler pipeline integration
- Connect with `vuma-parser` `to_scg` module (replace local SCG types with imports from this crate)
- Add `Eq` derive to `SCGError` and `ValidationResult` for testing convenience

## Task W1-14: Parser Crate Scaffold
**Date:** 2026-03-05
**Agent:** W1-14
**Status:** ✅ Complete

### Summary
Created the `vuma-parser` Rust crate — the VUMA language frontend — with full lexer, AST, recursive-descent parser, error reporting, and AST-to-SCG bridge.

### Files Created
| File | Description |
|------|-------------|
| `src/parser/Cargo.toml` | Crate manifest (deps: serde, thiserror, log) |
| `src/parser/src/lib.rs` | Root module with re-exports and integration tests |
| `src/parser/src/lexer.rs` | Tokeniser: `Token`/`TokenKind` enums, `Lexer` struct with `new()`, `next_token()`, `peek()`, span tracking, comment/whitespace skipping |
| `src/parser/src/ast.rs` | Full AST: `Program`, `Item`, `FnDef`, `Block`, `Stmt` (10 variants), `Expr` (13 variants), `Type` (5 variants), `Lit` (5 variants), `BinOp`/`UnOp` |
| `src/parser/src/error.rs` | `ParseError` with `Span`, `ParseErrorKind` (6 variants), `Display` with source context + pointer |
| `src/parser/src/parser.rs` | Recursive-descent parser with precedence climbing, error recovery (skip to `;`/`}`), `Item::Stmt` for top-level statements |
| `src/parser/src/to_scg.rs` | `AstToScg` converter: `SCG`, `ScgNode` (8 variants), `ScgEdge`/`EdgeKind`, scope tracking for DataFlow edges |

### Key Design Decisions
1. **Top-level statements allowed** — `Item::Stmt` variant permits assignments, `free()`, and expression statements at module scope, matching the VUMA example syntax.
2. **Comparison/logical operators added to lexer** — `==`, `!=`, `<`, `<=`, `>`, `>=`, `&&`, `||`, `!` are all lexed as distinct `TokenKind` variants.
3. **Local SCG types** — Since `vuma-scg` crate is empty, `to_scg.rs` defines its own `SCG`/`ScgNode`/`ScgEdge` types to be replaced with imports later.
4. **Borrow-after-move fixes** — Captured `expr.span().end` before moving `expr` into `Box::new(expr)`.

### Test Results
```
15 tests passed, 0 failed, 2 doc-tests passed
- lexer: address literal, arrow, string escapes, peek, comments
- parser: region def, fn def, cast expr, example program
- to_scg: simple region, fn def, example program
- integration: full pipeline (source→AST→SCG), import/export
```

### Next Actions
- Integrate with `vuma-scg` crate once its types are defined (replace local SCG types with imports)
- Add float literal support to lexer
- Add `true`/`false` boolean keyword tokens to lexer
- Add `LBrack`/`RBrack` token kinds for array indexing syntax
- Implement `Display` for `Program`/`Item`/`Stmt`/`Expr` for pretty-printing
