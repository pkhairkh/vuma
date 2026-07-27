//! SCG conformance / roundtrip regression test — Task 7-D.
//!
//! Documents the two-SCG divergence confirmed in Round 7 and recorded in
//! `PLAN_IVE_IR_DIVERGENCE.md`: the Invariant Verification Engine (IVE)
//! runs its soundness verifiers on the **semantic SCG** (`vuma-scg`;
//! `src/scg/src/node.rs`), which has 5 dedicated typed-state payload
//! kinds (`StateInit`, `StateRead`, `StateWrite`, `StateTransform`,
//! `ForeignConsume`). The emitted binary, however, is produced from a
//! **different IR** — the codegen SCG (`vuma_codegen::Scg`) — via the
//! canonical AST→codegen bridge `vuma::pipeline::bridge_ast_to_codegen_scg`,
//! which lowers typed-state ops to UNTYPED `AllocationNode::Stack` /
//! `AccessNode` / `CallNode` statements, **losing** the `layout_name` +
//! typed-state information that IVE reasons about.
//!
//! There are two bridges from a common parser AST:
//!   * AST → semantic SCG : `vuma_parser::AstToScg`              (IVE input)
//!   * AST → codegen  SCG : `vuma::pipeline::bridge_ast_to_codegen_scg` (binary producer)
//! plus a DEPRECATED semantic-SCG → codegen-SCG bridge
//!   * SCG  → codegen  SCG : `vuma::pipeline::bridge_scg_to_codegen`
//! which was abandoned (segfaults / infinite loops; see
//! `src/pipeline.rs:4892-4911`) but is retained because binaries / tests
//! still import it.
//!
//! These tests are REGRESSION TRIPWIRES: they pin the CURRENT divergence
//! and fail loudly if it widens OR narrows without an explicit code
//! change + `PLAN_IVE_IR_DIVERGENCE.md` revision.

use vuma_codegen::scg_to_ir::{PmtOpStmt, Scg, ScgNode, ScgStatement};
use vuma_scg::{
    ForeignConsumeNode, NodePayload, NodeType, ProgramPoint, SCG, StateInitNode, StateReadNode,
    StateTransformNode, StateWriteNode,
};

// ── helpers ─────────────────────────────────────────────────────────────

/// Build a minimal semantic SCG carrying exactly one node of each of the 5
/// typed-state payload kinds. This is the IVE-verified IR shape. We hand-build
/// it (rather than parsing source) because `AstToScg` does NOT emit
/// `StateTransform` for user-declared `transform`s — it lowers them to a
/// generic `Computation` node (see `PLAN_IVE_IR_DIVERGENCE.md` §3 row 2) — so
/// parsing cannot produce all 5 kinds.
fn build_semantic_scg_with_all_typed_state_payloads() -> SCG {
    let mut scg = SCG::new();
    let pp = ProgramPoint {
        file: None,
        line: None,
        column: None,
        offset: None,
    };

    // StateInit — `let p = state_new(Point)`
    scg.add_node(
        NodeType::StateInit,
        NodePayload::StateInit(StateInitNode {
            layout_name: "Point".to_string(),
            result_vreg: 0,
        }),
        pp.clone(),
    );
    // StateRead — `let x = p.x`
    scg.add_node(
        NodeType::StateRead,
        NodePayload::StateRead(StateReadNode {
            state_vreg: 0,
            layout_name: "Point".to_string(),
            field_name: "x".to_string(),
            result_vreg: 1,
        }),
        pp.clone(),
    );
    // StateWrite — `p.x = 42`
    scg.add_node(
        NodeType::StateWrite,
        NodePayload::StateWrite(StateWriteNode {
            state_vreg: 0,
            layout_name: "Point".to_string(),
            field_name: "x".to_string(),
            value_vreg: 2,
        }),
        pp.clone(),
    );
    // StateTransform — `parse : Raw -> Parsed` (the 5th typed-state kind;
    // never emitted by AstToScg for user transforms, so hand-built here).
    scg.add_node(
        NodeType::StateTransform,
        NodePayload::StateTransform(StateTransformNode {
            input_vreg: 3,
            input_layout: "Raw".to_string(),
            output_layout: "Parsed".to_string(),
            result_vreg: 4,
        }),
        pp.clone(),
    );
    // ForeignConsume — `consume(p)` / `#[foreign_consume]` close-call marker.
    scg.add_node(
        NodeType::ForeignConsume,
        NodePayload::ForeignConsume(ForeignConsumeNode {
            input_vreg: 0,
            layout_name: "Point".to_string(),
        }),
        pp,
    );
    scg
}

/// `[StateInit, StateRead, StateWrite, StateTransform, ForeignConsume]`
/// counts across all nodes of a semantic SCG.
fn count_semantic_typed_state(scg: &SCG) -> [usize; 5] {
    let mut c = [0usize; 5];
    for n in scg.nodes() {
        match n.payload {
            NodePayload::StateInit(_) => c[0] += 1,
            NodePayload::StateRead(_) => c[1] += 1,
            NodePayload::StateWrite(_) => c[2] += 1,
            NodePayload::StateTransform(_) => c[3] += 1,
            NodePayload::ForeignConsume(_) => c[4] += 1,
            _ => {}
        }
    }
    c
}

/// `[StateInit, StateRead, StateWrite, StateTransform, ForeignConsume]`
/// counts across all function bodies of a codegen SCG. The codegen SCG
/// represents typed-state ops via `ScgStatement::PmtOp(PmtOpStmt::*)`
/// (for the 4 state ops) and `ScgStatement::ForeignConsume(...)` (for the
/// consume marker). Any state op lowered to a generic `Allocation` /
/// `Access` / `Call` statement is NOT counted here — that is exactly the
/// information loss the divergence describes.
fn count_codegen_typed_state(cg: &Scg) -> [usize; 5] {
    let mut c = [0usize; 5];
    for node in &cg.nodes {
        if let ScgNode::Function(f) = node {
            for stmt in &f.body {
                match stmt {
                    ScgStatement::PmtOp(PmtOpStmt::StateInit { .. }) => c[0] += 1,
                    ScgStatement::PmtOp(PmtOpStmt::StateRead { .. }) => c[1] += 1,
                    ScgStatement::PmtOp(PmtOpStmt::StateWrite { .. }) => c[2] += 1,
                    ScgStatement::PmtOp(PmtOpStmt::StateTransform { .. }) => c[3] += 1,
                    ScgStatement::ForeignConsume(_) => c[4] += 1,
                    _ => {}
                }
            }
        }
    }
    c
}

// ── tests ───────────────────────────────────────────────────────────────

/// Task 7-D step 1 — the semantic SCG (IVE input) can represent ALL 5
/// typed-state payload kinds, each carrying its typed layout/field info.
/// This is the IVE-verified side of the divergence.
#[test]
fn semantic_scg_carries_all_five_typed_state_payloads() {
    let scg = build_semantic_scg_with_all_typed_state_payloads();

    assert_eq!(scg.node_count(), 5);
    let counts = count_semantic_typed_state(&scg);
    assert_eq!(
        counts,
        [1, 1, 1, 1, 1],
        "semantic SCG must carry exactly one of each typed-state payload kind \
         [StateInit, StateRead, StateWrite, StateTransform, ForeignConsume]"
    );

    // The `layout_name` is the IVE-relevant typed-state info that the
    // codegen side loses. Verify each payload carries it through.
    let mut layouts: Vec<&str> = scg
        .nodes()
        .filter_map(|n| match &n.payload {
            NodePayload::StateInit(s) => Some(s.layout_name.as_str()),
            NodePayload::StateRead(s) => Some(s.layout_name.as_str()),
            NodePayload::StateWrite(s) => Some(s.layout_name.as_str()),
            NodePayload::StateTransform(s) => Some(s.input_layout.as_str()),
            NodePayload::ForeignConsume(s) => Some(s.layout_name.as_str()),
            _ => None,
        })
        .collect();
    layouts.sort_unstable();
    assert_eq!(
        layouts,
        ["Point", "Point", "Point", "Point", "Raw"],
        "each typed-state payload must carry its layout_name (IVE-relevant typed info)"
    );
}

/// Task 7-D step 2 (SCG→codegen path) — the DEPRECATED
/// `bridge_scg_to_codegen` (the only semantic-SCG → codegen-SCG bridge)
/// PRESERVES typed-state info: it lowers each typed payload to the codegen
/// SCG's `PmtOp(StateInit|StateRead|StateWrite|StateTransform)` /
/// `ForeignConsume` variants, carrying `layout_name` through. This bridge
/// is NOT the binary producer (it was abandoned for segfaults / infinite
/// loops — see `src/pipeline.rs:4892-4911`); the binary producer is
/// `bridge_ast_to_codegen_scg`, which LOSES typed-state info (see the next
/// test). This test pins the deprecated bridge's info-preserving behavior
/// so any drift is caught.
#[test]
fn deprecated_scg_to_codegen_bridge_preserves_typed_state() {
    let scg = build_semantic_scg_with_all_typed_state_payloads();

    let cg = vuma::pipeline::bridge_scg_to_codegen(&scg);
    let counts = count_codegen_typed_state(&cg);

    // The deprecated SCG→codegen bridge roundtrips all 5 typed-state
    // payloads (4 PmtOp + 1 ForeignConsume), preserving layout_name. If
    // this assertion fails, the deprecated bridge's lowering of typed-state
    // nodes has drifted — update PLAN_IVE_IR_DIVERGENCE.md §3 + this test.
    assert_eq!(
        counts,
        [1, 1, 1, 1, 1],
        "deprecated SCG→codegen bridge must preserve all 5 typed-state payloads \
         ([StateInit, StateRead, StateWrite, StateTransform, ForeignConsume] as \
         PmtOp variants + ForeignConsume). Drift = bridge lowering changed."
    );
}

/// Task 7-D step 2+3 (canonical AST→codegen path) — the CANONICAL binary
/// producer `bridge_ast_to_codegen_scg` lowers `state_new(L)` / `p.field`
/// read/write to UNTYPED `AllocationNode::Stack` / `AccessNode::Store|Load`
/// statements, losing the `layout_name` + typed-state information. This is
/// the Round 7 divergence: IVE verifies the typed semantic SCG, but the
/// binary is produced from this UNTYPED codegen SCG. This test ASSERTS the
/// divergence and fails if it narrows (bridge starts emitting typed
/// `PmtOp` payloads) or the source stops parsing — both require an
/// explicit code + `PLAN_IVE_IR_DIVERGENCE.md` update.
#[test]
fn canonical_ast_bridge_loses_typed_state_divergence_documented() {
    // Source exercises state_new (semantic StateInit), field write
    // (semantic StateWrite), field read (semantic StateRead). AstToScg
    // emits typed payloads for these on the semantic side; the codegen
    // bridge must NOT emit typed `PmtOp` payloads for them (it lowers to
    // untyped Allocation/Access). StateTransform + ForeignConsume are not
    // exercised by this source (see
    // `semantic_scg_carries_all_five_typed_state_payloads` for the full 5).
    let src = "\
layout Point = { x: u32, y: u32 }

fn main() -> i32 {
    let p = state_new(Point);
    p.x = 42;
    let v = p.x;
    return 0;
}
";
    let mut parser = vuma_parser::Parser::new(src);
    let parse_output = parser.parse_program();
    assert!(
        !parse_output.has_errors(),
        "minimal state_new source must parse; errors: {:?}",
        parse_output.errors
    );
    let ast = parse_output.unwrap();

    let cg = vuma::pipeline::bridge_ast_to_codegen_scg(&ast);
    let counts = count_codegen_typed_state(&cg);

    // DIVERGENCE DOCUMENTED (option b): the canonical AST→codegen bridge
    // emits ZERO typed `PmtOp::StateInit|StateRead|StateWrite` payloads for
    // user state_new / field ops — it lowers them to untyped
    // `AllocationNode::Stack` / `AccessNode`. IVE verifies the typed
    // semantic SCG; the binary is produced from this untyped codegen SCG.
    //
    // If any of counts[0..3] becomes nonzero, the bridge started
    // preserving typed-state info (divergence NARROWED) — flip this test
    // to assert preservation and update PLAN_IVE_IR_DIVERGENCE.md §3.
    assert_eq!(counts[0], 0, "StateInit must lower to untyped Allocation (divergence)");
    assert_eq!(counts[1], 0, "StateRead must lower to untyped Access/Load (divergence)");
    assert_eq!(counts[2], 0, "StateWrite must lower to untyped Access/Store (divergence)");
    assert_eq!(counts[3], 0, "StateTransform not emitted by this source");
    assert_eq!(counts[4], 0, "ForeignConsume not emitted by this source");
}
