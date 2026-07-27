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
//! The deprecated semantic-SCG → codegen-SCG bridge
//! (`bridge_scg_to_codegen`) has been removed; the canonical path is now
//! exclusively the AST→codegen bridge above.
//!
//! These tests are REGRESSION TRIPWIRES: they pin the CURRENT divergence
//! and fail loudly if it widens OR narrows without an explicit code
//! change + `PLAN_IVE_IR_DIVERGENCE.md` revision.

use vuma_codegen::scg_to_ir::{PmtOpStmt, Scg, ScgNode, ScgStatement, TypedStateMeta};
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

/// Task 2-B (canonical AST→codegen path) — the CANONICAL binary producer
/// `bridge_ast_to_codegen_scg` lowers `state_new(L)` / `p.field` read/write
/// to UNTYPED `AllocationNode::Stack` / `AccessNode::Store|Load` statements,
/// so the emitted codegen SCG nodes carry NO typed-state payload (the binary
/// is byte-identical to pre-SCG-CLOSURE). BUT the bridge now ALSO produces
/// recoverable `TypedStateMeta` metadata as a side product of its single
/// canonical AST walk — the standalone `extract_typed_state_meta_from_ast`
/// parallel walker was merged into the bridge in Task 2-A and deleted in
/// Task 2-B, so there is no longer a DUPLICATE AST walk.
///
/// **Task SCG-CLOSURE — typed-state metadata PRESERVED.** The divergence
/// narrows from "5 typed-state payloads LOST" to "0 lost — preserved as
/// `TypedStateMeta` metadata threaded through the bridge itself": IVE *could*
/// replay this metadata against the codegen SCG to recover the typed-state
/// view it verifies on the semantic SCG.
///
/// This test asserts BOTH properties of the merged bridge:
///   * the codegen SCG's `PmtOp` typed-state counts are STILL 0 (the bridge
///     still lowers to untyped Allocation/Access — binary unchanged); AND
///   * the `TypedStateMeta` metadata returned by
///     `bridge_ast_to_codegen_scg_with_meta` recovers all 3 typed-state ops
///     the Point source exercises (StateInit{Point}, StateWrite{p,Point,x},
///     StateRead{p,Point,x}), with the `layout_name` preserved.
///
/// `StateTransform` + `ForeignConsume` are not exercised by this source
/// (covered by `semantic_scg_carries_all_five_typed_state_payloads` for the
/// full 5-kind shape).
#[test]
fn canonical_ast_bridge_preserves_typed_state_metadata() {
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

    // Single canonical AST walk: the bridge produces the codegen SCG whose
    // first-class `typed_state_meta` field (Task 3-A) carries the recoverable
    // typed-state info as a side product of lowering. No separate parallel
    // walk, and no tuple to destructure -- the metadata is read directly
    // off the `Scg` struct.
    let cg = vuma::pipeline::bridge_ast_to_codegen_scg(&ast);
    let meta = &cg.typed_state_meta;

    // ── Side A (UNCHANGED): the codegen SCG's emitted nodes are STILL
    //    untyped. The canonical AST→codegen bridge emits ZERO typed
    //    `PmtOp::StateInit|StateRead|StateWrite` payloads for user
    //    state_new / field ops — it lowers them to untyped
    //    `AllocationNode::Stack` / `AccessNode`. The binary is byte-identical
    //    to pre-SCG-CLOSURE. If any of counts[0..3] becomes nonzero, the
    //    bridge started emitting typed payloads directly (divergence FULLY
    //    closed at the node level) — flip this block to assert preservation
    //    and update PLAN_IVE_IR_DIVERGENCE.md §3.
    let counts = count_codegen_typed_state(&cg);
    assert_eq!(counts[0], 0, "StateInit still lowers to untyped Allocation");
    assert_eq!(counts[1], 0, "StateRead still lowers to untyped Access/Load");
    assert_eq!(counts[2], 0, "StateWrite still lowers to untyped Access/Store");
    assert_eq!(counts[3], 0, "StateTransform not emitted by this source");
    assert_eq!(counts[4], 0, "ForeignConsume not emitted by this source");

    // ── Side B (SCG-CLOSURE): the typed-state info is PRESERVED as
    //    recoverable `TypedStateMeta` metadata, now produced by the bridge's
    //    own canonical walk (merged in Task 2-A; standalone extractor deleted
    //    in Task 2-B). One entry per typed-state op the bridge lowered away,
    //    so the `layout_name` that Side A lost at the node level is recovered
    //    here at the metadata level.

    // `[StateInit, StateRead, StateWrite, StateTransform, ForeignConsume]`
    // counts across the recovered metadata.
    let mut mc = [0usize; 5];
    for m in meta {
        match m {
            TypedStateMeta::StateInit { .. } => mc[0] += 1,
            TypedStateMeta::StateRead { .. } => mc[1] += 1,
            TypedStateMeta::StateWrite { .. } => mc[2] += 1,
            TypedStateMeta::StateTransform { .. } => mc[3] += 1,
            TypedStateMeta::ForeignConsume { .. } => mc[4] += 1,
        }
    }
    assert_eq!(
        mc,
        [1, 1, 1, 0, 0],
        "TypedStateMeta must recover exactly the 3 typed-state ops the Point \
         source exercises ([StateInit, StateRead, StateWrite] = [1,1,1]; \
         StateTransform + ForeignConsume not exercised). If this fails, the \
         bridge's typed-state recognition drifted."
    );

    // ── Side B (cont.): the recovered metadata carries the IVE-relevant
    //    typed info (layout_name + field_name + result_vreg) that Side A
    //    lost. Verify each entry's payload so the metadata is structurally
    //    faithful to the semantic SCG's typed-state payloads.
    let mut found_init = false;
    let mut found_read = false;
    let mut found_write = false;
    for m in meta {
        match m {
            TypedStateMeta::StateInit { layout_name, result_vreg } => {
                assert_eq!(
                    layout_name, "Point",
                    "StateInit metadata must preserve the Point layout_name"
                );
                assert_eq!(
                    *result_vreg, 0,
                    "StateInit metadata's synthetic vreg must be 0 for the first state op"
                );
                found_init = true;
            }
            TypedStateMeta::StateRead { var, layout_name, field_name } => {
                assert_eq!(var, "p", "StateRead metadata var must be p");
                assert_eq!(
                    layout_name, "Point",
                    "StateRead metadata must preserve the Point layout_name"
                );
                assert_eq!(field_name, "x", "StateRead metadata field must be x");
                found_read = true;
            }
            TypedStateMeta::StateWrite { var, layout_name, field_name } => {
                assert_eq!(var, "p", "StateWrite metadata var must be p");
                assert_eq!(
                    layout_name, "Point",
                    "StateWrite metadata must preserve the Point layout_name"
                );
                assert_eq!(field_name, "x", "StateWrite metadata field must be x");
                found_write = true;
            }
            TypedStateMeta::StateTransform { .. } | TypedStateMeta::ForeignConsume { .. } => {
                // Not exercised by this source — counted above.
            }
        }
    }
    assert!(found_init && found_read && found_write,
        "metadata must contain one StateInit, one StateRead, one StateWrite");
}

/// Task 3-B / 3-C -- the IVE typed-state conformance cross-check is wired
/// into the full pipeline and runs WITHOUT breaking it. The cross-check
/// (Task 3-B, `verify_typed_state_conformance`) is warn-only: on a mismatch
/// it logs `[Task 3-B] typed-state conformance cross-check ...` and does
/// NOT hard-fail, so the pipeline must still return `Ok`. This test proves
/// the wiring end-to-end by compiling the same Point state-ops source test
/// 3 uses through the full `vuma::pipeline::compile` path and asserting
/// success -- i.e. the IVE cross-check is exercised on the canonical
/// AST->codegen bridge output (whose `typed_state_meta` is now first-class
/// on the `Scg`) without aborting compilation.
#[test]
fn ive_cross_check_passes_for_canonical_bridge() {
    // Same Point state-ops source as
    // `canonical_ast_bridge_preserves_typed_state_metadata` -- exercises
    // state_new (StateInit), field write (StateWrite), field read
    // (StateRead): the three typed-state ops the IVE cross-check
    // reconciles between the semantic SCG and the codegen SCG. A raw
    // string literal keeps the source verbatim (no line-continuation
    // backslashes needed).
    let src = r#"layout Point = { x: u32, y: u32 }

fn main() -> i32 {
    let p = state_new(Point);
    p.x = 42;
    let v = p.x;
    return 0;
}
"#;
    let config = vuma::pipeline::CompileConfig::default();
    let result = vuma::pipeline::compile(src, &config);
    assert!(
        result.is_ok(),
        "full pipeline must succeed with the IVE typed-state conformance \
         cross-check wired in (Task 3-B is warn-only, never hard-fails). \
         errors: {:?}",
        result.err()
    );
}
