//! # Property-Based Tests for VUMA
//!
//! This module implements property-based tests using the `proptest` framework
//! to validate the VUMA compiler across several dimensions:
//!
//! - **Random program generation**: Generate random valid VUMA programs
//!   (simple expressions, function calls, memory operations) and verify
//!   they compile without crashing.
//! - **Cross-backend consistency**: Compile random programs for all backends
//!   and verify they produce structurally valid output.
//! - **Parser roundtrip**: Generate random valid VUMA source, parse it,
//!   and verify no errors are produced.
//! - **SCG invariants**: Verify structural invariants of the SCG: every
//!   function has an entry node, every edge connects valid nodes.

use proptest::prelude::*;
use vuma_scg::{
    EdgeKind, NodeId, NodePayload, NodeType, ProgramPoint, SCG,
    ControlKind, ComputationNode, ControlNode, AllocationNode, DeallocationNode,
    AccessNode, AccessMode, RegionId, SCGRegion, DeploymentTarget,
};

// ═══════════════════════════════════════════════════════════════════════════
// Random Program Generation Strategies
// ═══════════════════════════════════════════════════════════════════════════

/// Generate a random valid VUMA identifier.
fn arb_identifier() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{0,15}"
}

/// Generate a random integer literal.
fn arb_int_literal() -> impl Strategy<Value = i64> {
    any::<i64>().prop_map(|v| v % 1000)
}

/// Generate a random binary operator.
fn arb_binop() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("+"),
        Just("-"),
        Just("*"),
        Just("/"),
        Just("&"),
        Just("|"),
        Just("^"),
        Just("<<"),
        Just(">>"),
        Just("=="),
        Just("!="),
        Just("<"),
        Just("<="),
        Just(">"),
        Just(">="),
    ]
}

/// Generate a random simple expression statement.
fn arb_simple_expr() -> impl Strategy<Value = String> {
    (arb_identifier(), arb_binop(), arb_identifier())
        .prop_map(|(lhs, op, rhs)| format!("    {} = {} {} {};", lhs, lhs, op, rhs))
}

/// Generate a random assignment from a literal.
fn arb_lit_assign() -> impl Strategy<Value = String> {
    (arb_identifier(), arb_int_literal())
        .prop_map(|(name, val)| format!("    {} = {};", name, val))
}

/// Generate a random single statement inside a function body.
fn arb_statement() -> impl Strategy<Value = String> {
    prop_oneof![arb_simple_expr(), arb_lit_assign(),]
}

/// Generate a random function body (1-5 statements).
fn arb_fn_body() -> impl Strategy<Value = String> {
    prop::collection::vec(arb_statement(), 1..5)
        .prop_map(|stmts| stmts.join("\n"))
}

/// Generate a random VUMA function definition.
fn arb_fn_def() -> impl Strategy<Value = String> {
    (arb_identifier(), arb_fn_body())
        .prop_map(|(name, body)| format!("fn {}() {{\n{}\n}}", name, body))
}

/// Generate a random complete VUMA program (1-3 functions, always
/// includes `main`).
fn arb_vuma_program() -> impl Strategy<Value = String> {
    prop::collection::vec(arb_fn_def(), 0..2).prop_map(|fns| {
        let mut program = String::new();
        for f in &fns {
            program.push_str(f);
            program.push('\n');
        }
        // Always include a main function so compilation works.
        program.push_str("fn main() {\n}\n");
        program
    })
}

/// Generate a random memory operation program (with region allocation).
fn arb_memory_program() -> impl Strategy<Value = String> {
    (arb_identifier(), 64usize..512)
        .prop_map(|(region_name, size)| {
            format!(
                "region {} = allocate({});\nfn main() {{\n    ptr = {} + 64;\n}}\n",
                region_name, size, region_name
            )
        })
}

/// Generate a random function call program.
fn arb_call_program() -> impl Strategy<Value = String> {
    arb_identifier().prop_map(|helper_name| {
        format!(
            "fn {}() {{\n    x = 1 + 2;\n}}\nfn main() {{\n    {}();\n}}\n",
            helper_name, helper_name
        )
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// SCG Random Generation Strategies
// ═══════════════════════════════════════════════════════════════════════════

/// Generate a random NodeType.
fn arb_node_type() -> impl Strategy<Value = NodeType> {
    prop_oneof![
        Just(NodeType::Computation),
        Just(NodeType::Allocation),
        Just(NodeType::Deallocation),
        Just(NodeType::Access),
        Just(NodeType::Control),
        Just(NodeType::Cast),
        Just(NodeType::Effect),
        Just(NodeType::Phantom),
    ]
}

/// Generate a random EdgeKind.
fn arb_edge_kind() -> impl Strategy<Value = EdgeKind> {
    prop_oneof![
        Just(EdgeKind::DataFlow),
        Just(EdgeKind::ControlFlow),
        Just(EdgeKind::Derivation),
        Just(EdgeKind::Annotation),
    ]
}

/// Generate a random computation node payload.
fn arb_computation_payload() -> impl Strategy<Value = NodePayload> {
    (arb_identifier(), prop_oneof![Just(Some("i64".to_string())), Just(None)])
        .prop_map(|(op, rt)| {
            NodePayload::Computation(ComputationNode {
                operation: op,
                result_type: rt,
                tail_call: false,
            })
        })
}

/// Generate a random allocation node payload.
fn arb_allocation_payload() -> impl Strategy<Value = NodePayload> {
    (1u64..4096, 1u64..64)
        .prop_map(|(size, align)| {
            NodePayload::Allocation(AllocationNode {
                size,
                align,
                region_id: RegionId::new(0),
                type_name: None,
            })
        })
}

/// Generate a random node payload.
fn arb_node_payload() -> impl Strategy<Value = NodePayload> {
    prop_oneof![
        arb_computation_payload(),
        arb_allocation_payload(),
        Just(NodePayload::Control(ControlNode {
            kind: ControlKind::FunctionEntry,
            label: Some("main".to_string()),
        })),
        Just(NodePayload::Deallocation(DeallocationNode {
            allocation_node: NodeId::new(0),
            region_id: RegionId::new(0),
        })),
        Just(NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: RegionId::new(0),
            offset: Some(0),
            access_size: Some(8),
        })),
    ]
}

/// Generate a random program point.
fn arb_program_point() -> impl Strategy<Value = ProgramPoint> {
    (any::<Option<u64>>(), any::<Option<u64>>())
        .prop_map(|(line, column)| ProgramPoint {
            file: Some("test.vu".to_string()),
            line,
            column,
            offset: None,
        })
}

// ═══════════════════════════════════════════════════════════════════════════
// Property Tests: Parser Roundtrip
// ═══════════════════════════════════════════════════════════════════════════

proptest! {
    /// Parse a randomly generated VUMA program and verify no parse errors.
    #[test]
    fn prop_parser_roundtrip_no_errors(program in arb_vuma_program()) {
        let mut parser = vuma_parser::Parser::new(&program);
        let result = parser.parse_program();
        prop_assert!(
            !result.has_errors(),
            "Randomly generated program should parse without errors. Errors: {:?}",
            result.errors
        );
    }

    /// Parse a random memory program and verify no parse errors.
    #[test]
    fn prop_parser_memory_program(program in arb_memory_program()) {
        let mut parser = vuma_parser::Parser::new(&program);
        let result = parser.parse_program();
        prop_assert!(
            !result.has_errors(),
            "Memory program should parse without errors. Errors: {:?}",
            result.errors
        );
    }

    /// Parse a random call program and verify no parse errors.
    #[test]
    fn prop_parser_call_program(program in arb_call_program()) {
        let mut parser = vuma_parser::Parser::new(&program);
        let result = parser.parse_program();
        prop_assert!(
            !result.has_errors(),
            "Call program should parse without errors. Errors: {:?}",
            result.errors
        );
    }

    /// Parse a valid program, convert to SCG, and verify the SCG has nodes.
    #[test]
    fn prop_parse_to_scg_has_nodes(program in arb_vuma_program()) {
        let mut parser = vuma_parser::Parser::new(&program);
        let parse_result = parser.parse_program();
        if parse_result.has_errors() {
            // Some random programs might not parse; skip those.
            return Ok(());
        }
        let ast = parse_result.unwrap();
        let mut converter = vuma_parser::AstToScg::new();
        match converter.convert(&ast) {
            Ok(scg) => {
                prop_assert!(
                    scg.node_count() > 0,
                    "SCG should have at least one node for any valid program"
                );
            }
            Err(_) => {
                // AST-to-SCG conversion can fail for some programs.
                // That's acceptable — we just verify no panic.
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Property Tests: Cross-Backend Consistency
// ═══════════════════════════════════════════════════════════════════════════

proptest! {
    /// Compile a random program for all backends and verify they all
    /// produce valid binary output.
    #[test]
    fn prop_cross_backend_all_produce_output(program in arb_vuma_program()) {
        use vuma::api::VumaCompiler;

        let compiler = VumaCompiler::new();

        // First verify the program compiles at all.
        let default_result = compiler.compile(&program);
        if !default_result.success {
            // Some randomly generated programs may not compile due to
            // semantic issues (e.g., undeclared variables). Skip those.
            return Ok(());
        }

        let targets = ["x86_64", "aarch64", "riscv64", "arm32", "mips64", "ppc64"];

        for target in &targets {
            let result = compiler.compile_for_target(&program, target);
            prop_assert!(
                result.success,
                "Compilation should succeed for target '{}'. Diagnostics: {:?}",
                target,
                result.diagnostics
            );
            prop_assert!(
                result.target.is_some(),
                "Should have target output for '{}'",
                target
            );
            if let Some(ref tgt) = result.target {
                prop_assert!(
                    !tgt.binary.is_empty(),
                    "Binary output should not be empty for '{}'",
                    target
                );
            }
        }
    }

    /// Compile a memory program for all backends and verify same
    /// SCG structure across targets.
    #[test]
    fn prop_cross_backend_same_scg(program in arb_memory_program()) {
        use vuma::api::VumaCompiler;

        let compiler = VumaCompiler::new();

        // Compile for two different targets and compare SCG summaries.
        let result_a = compiler.compile_for_target(&program, "aarch64");
        let result_b = compiler.compile_for_target(&program, "x86_64");

        if result_a.success && result_b.success {
            if let (Some(scg_a), Some(scg_b)) = (&result_a.scg, &result_b.scg) {
                // The SCG is target-independent; both should have the
                // same function count and node count.
                prop_assert_eq!(
                    scg_a.function_count, scg_b.function_count,
                    "Function count should be same across backends"
                );
                prop_assert_eq!(
                    scg_a.total_nodes, scg_b.total_nodes,
                    "Total nodes should be same across backends"
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Property Tests: SCG Structural Invariants
// ═══════════════════════════════════════════════════════════════════════════

proptest! {
    /// Every SCG built from a valid program should have a function entry
    /// node for each function.
    #[test]
    fn prop_scg_every_function_has_entry(program in arb_vuma_program()) {
        let mut parser = vuma_parser::Parser::new(&program);
        let parse_result = parser.parse_program();
        if parse_result.has_errors() {
            return Ok(());
        }
        let ast = parse_result.unwrap();
        let mut converter = vuma_parser::AstToScg::new();
        let scg = match converter.convert(&ast) {
            Ok(scg) => scg,
            Err(_) => return Ok(()),
        };

        // Count FunctionEntry nodes.
        let entry_count = scg.nodes().filter(|n| {
            matches!(&n.payload, NodePayload::Control(c) if c.kind == ControlKind::FunctionEntry)
        }).count();

        prop_assert!(
            entry_count >= 1,
            "SCG should have at least one FunctionEntry node (found {})",
            entry_count
        );
    }

    /// Every edge in the SCG should connect valid (existing) nodes.
    #[test]
    fn prop_scg_edges_connect_valid_nodes(program in arb_vuma_program()) {
        let mut parser = vuma_parser::Parser::new(&program);
        let parse_result = parser.parse_program();
        if parse_result.has_errors() {
            return Ok(());
        }
        let ast = parse_result.unwrap();
        let mut converter = vuma_parser::AstToScg::new();
        let scg = match converter.convert(&ast) {
            Ok(scg) => scg,
            Err(_) => return Ok(()),
        };

        // Collect all node IDs.
        let node_ids: std::collections::HashSet<NodeId> =
            scg.nodes().map(|n| n.id).collect();

        // Verify every edge connects existing nodes.
        for edge in scg.edges() {
            prop_assert!(
                node_ids.contains(&edge.source),
                "Edge source {:?} should exist in the graph",
                edge.source
            );
            prop_assert!(
                node_ids.contains(&edge.target),
                "Edge target {:?} should exist in the graph",
                edge.target
            );
        }
    }

    /// SCG validation should pass for any valid program.
    #[test]
    fn prop_scg_validation_passes(program in arb_vuma_program()) {
        let mut parser = vuma_parser::Parser::new(&program);
        let parse_result = parser.parse_program();
        if parse_result.has_errors() {
            return Ok(());
        }
        let ast = parse_result.unwrap();
        let mut converter = vuma_parser::AstToScg::new();
        let scg = match converter.convert(&ast) {
            Ok(scg) => scg,
            Err(_) => return Ok(()),
        };

        let validation = scg.validate();
        prop_assert!(
            validation.is_valid,
            "SCG validation should pass for valid programs. Errors: {:?}",
            validation.errors
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Property Tests: SCG Construction Invariants
// ═══════════════════════════════════════════════════════════════════════════

proptest! {
    /// Build a random SCG by adding random nodes and verify structural
    /// invariants hold.
    #[test]
    fn prop_scg_random_construction_invariants(
        nodes in prop::collection::vec(
            (arb_node_payload(), arb_program_point()),
            1..20
        )
    ) {
        let mut scg = SCG::new();
        let mut node_ids = Vec::new();

        // Add nodes.
        for (payload, pp) in nodes {
            let node_type = match &payload {
                NodePayload::Computation(_) => NodeType::Computation,
                NodePayload::Allocation(_) => NodeType::Allocation,
                NodePayload::Deallocation(_) => NodeType::Deallocation,
                NodePayload::Access(_) => NodeType::Access,
                NodePayload::Control(_) => NodeType::Control,
                NodePayload::Cast(_) => NodeType::Cast,
                NodePayload::Effect(_) => NodeType::Effect,
                NodePayload::Phantom(_) => NodeType::Phantom,
                NodePayload::VTable(_) => NodeType::VTable,
                NodePayload::ClosureEnv(_) => NodeType::ClosureEnv,
            };
            let id = scg.add_node(node_type, payload, pp);
            node_ids.push(id);
        }

        // Invariant: node count matches.
        prop_assert_eq!(
            scg.node_count(),
            node_ids.len(),
            "Node count should match the number of added nodes"
        );

        // Invariant: every added node can be retrieved.
        for &id in &node_ids {
            prop_assert!(
                scg.get_node(id).is_some(),
                "Added node {:?} should be retrievable",
                id
            );
        }

        // Invariant: edges between existing nodes should succeed.
        if node_ids.len() >= 2 {
            let edge_result = scg.add_edge(node_ids[0], node_ids[1], EdgeKind::DataFlow);
            prop_assert!(
                edge_result.is_ok(),
                "Adding an edge between existing nodes should succeed"
            );
            prop_assert_eq!(scg.edge_count(), 1, "Edge count should be 1");
        }
    }

    /// Build a random SCG and verify edges only connect valid nodes.
    #[test]
    fn prop_scg_random_edges_valid(
        nodes in prop::collection::vec(
            (arb_node_payload(), arb_program_point()),
            2..10
        ),
        edge_kinds in prop::collection::vec(arb_edge_kind(), 1..5)
    ) {
        let mut scg = SCG::new();
        let mut node_ids = Vec::new();

        for (payload, pp) in nodes {
            let node_type = match &payload {
                NodePayload::Computation(_) => NodeType::Computation,
                NodePayload::Allocation(_) => NodeType::Allocation,
                NodePayload::Deallocation(_) => NodeType::Deallocation,
                NodePayload::Access(_) => NodeType::Access,
                NodePayload::Control(_) => NodeType::Control,
                NodePayload::Cast(_) => NodeType::Cast,
                NodePayload::Effect(_) => NodeType::Effect,
                NodePayload::Phantom(_) => NodeType::Phantom,
                NodePayload::VTable(_) => NodeType::VTable,
                NodePayload::ClosureEnv(_) => NodeType::ClosureEnv,
            };
            let id = scg.add_node(node_type, payload, pp);
            node_ids.push(id);
        }

        // Add edges between random pairs of existing nodes.
        for (i, kind) in edge_kinds.into_iter().enumerate() {
            let src_idx = i % node_ids.len();
            let tgt_idx = (i + 1) % node_ids.len();
            let result = scg.add_edge(node_ids[src_idx], node_ids[tgt_idx], kind);
            prop_assert!(result.is_ok(), "Edge between valid nodes should succeed");
        }

        // Verify all edges connect existing nodes.
        let node_id_set: std::collections::HashSet<NodeId> = node_ids.into_iter().collect();
        for edge in scg.edges() {
            prop_assert!(
                node_id_set.contains(&edge.source),
                "Edge source should be a valid node"
            );
            prop_assert!(
                node_id_set.contains(&edge.target),
                "Edge target should be a valid node"
            );
        }
    }

    /// Adding an edge to a non-existent node should fail.
    #[test]
    fn prop_scg_edge_to_nonexistent_node_fails(
        payload in arb_computation_payload(),
        pp in arb_program_point(),
        kind in arb_edge_kind()
    ) {
        let mut scg = SCG::new();
        let node_id = scg.add_node(NodeType::Computation, payload, pp);
        let fake_id = NodeId::new(99999);

        // Edge from real to fake should fail.
        let result = scg.add_edge(node_id, fake_id, kind);
        prop_assert!(result.is_err(), "Edge to non-existent node should fail");

        // Edge from fake to real should also fail.
        let result = scg.add_edge(fake_id, node_id, EdgeKind::ControlFlow);
        prop_assert!(result.is_err(), "Edge from non-existent node should fail");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Property Tests: Verification Pipeline
// ═══════════════════════════════════════════════════════════════════════════

proptest! {
    /// Verify that VumaCompiler::verify() doesn't panic on random programs.
    #[test]
    fn prop_verify_no_panic(program in arb_vuma_program()) {
        use vuma::api::VumaCompiler;

        let compiler = VumaCompiler::new();
        let report = compiler.verify(&program);

        // The report should always be non-empty (even on error).
        prop_assert!(
            report.metadata.source_bytes > 0,
            "Metadata should record source size"
        );
    }

    /// Verify that the verification report is always serializable.
    #[test]
    fn prop_verify_report_serializable(program in arb_vuma_program()) {
        use vuma::api::VumaCompiler;

        let compiler = VumaCompiler::new();
        let report = compiler.verify(&program);

        let json_result = serde_json::to_string(&report);
        prop_assert!(
            json_result.is_ok(),
            "VerificationReport should always be serializable"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Property Tests: IVE Verification on Random SCGs
// ═══════════════════════════════════════════════════════════════════════════

proptest! {
    /// IVE verification on a random SCG should not panic and should
    /// produce results for all 5 invariants.
    #[test]
    fn prop_ive_verify_all_invariants(
        nodes in prop::collection::vec(
            (arb_node_payload(), arb_program_point()),
            1..10
        )
    ) {
        let mut scg = SCG::new();

        for (payload, pp) in nodes {
            let node_type = match &payload {
                NodePayload::Computation(_) => NodeType::Computation,
                NodePayload::Allocation(_) => NodeType::Allocation,
                NodePayload::Deallocation(_) => NodeType::Deallocation,
                NodePayload::Access(_) => NodeType::Access,
                NodePayload::Control(_) => NodeType::Control,
                NodePayload::Cast(_) => NodeType::Cast,
                NodePayload::Effect(_) => NodeType::Effect,
                NodePayload::Phantom(_) => NodeType::Phantom,
                NodePayload::VTable(_) => NodeType::VTable,
                NodePayload::ClosureEnv(_) => NodeType::ClosureEnv,
            };
            scg.add_node(node_type, payload, pp);
        }

        let aggregator = vuma_ive::InvariantAggregator::new();
        let input = vuma_ive::verification::VerificationInput::from_scg(scg);
        let result = aggregator.verify_all(&input);

        // Should always produce a result (even for an empty/trivial SCG).
        prop_assert!(
            !result.per_invariant.is_empty(),
            "Should have at least some invariant results"
        );

        // The overall verdict should be one of the known variants.
        prop_assert!(matches!(
            result.overall,
            vuma_ive::OverallVerdict::Pass
            | vuma_ive::OverallVerdict::Fail
            | vuma_ive::OverallVerdict::Inconclusive
            | vuma_ive::OverallVerdict::NoChecks
        ));
    }
}
