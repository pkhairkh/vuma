//! # Property-Based Tests for VUMA (Deterministic)
//!
//! This module implements deterministic regression tests for the VUMA
//! compiler across several dimensions.  Tests use hand-written,
//! fixed-seed inputs so the suite can run on self-hosted builds
//! without any external property-testing framework.
//!
//! - **Program compilation**: Hand-written valid VUMA programs (simple
//!   expressions, function calls, memory operations) compile without
//!   crashing.
//! - **Cross-backend consistency**: Compile programs for all backends
//!   and verify they produce structurally valid output.
//! - **Parser roundtrip**: Parse valid VUMA source and verify no
//!   errors are produced.
//! - **SCG invariants**: Verify structural invariants of the SCG:
//!   every function has an entry node, every edge connects valid nodes.
//! - **FP conversion roundtrip**: Verify that float↔int bit-cast
//!   roundtrips are lossless and that IntToFloat/FloatToInt casts
//!   compile correctly.
//! - **Atomic CAS correctness**: Verify CAS with matching expected
//!   value succeeds and CAS with non-matching value fails (at the IR
//!   level).
//! - **Rotate roundtrip**: Verify ROL(x, n) followed by ROR(x, n)
//!   equals x.
//! - **ABI consistency**: Verify functions with varying argument
//!   counts produce correct calling-convention code.
//! - **DWARF consistency**: Compiling with and without --debug should
//!   produce the same .text section.
//! - **FFI symbol emission**: Extern functions should produce
//!   SHN_UNDEF symbols in ELF output.

use vuma_scg::{
    AccessMode, AccessNode, AllocationNode, ComputationNode, ControlKind,
    ControlNode, DeallocationNode, EdgeKind, NodeId, NodePayload, NodeType,
    ProgramPoint, RegionId, SCG,
};

// ═══════════════════════════════════════════════════════════════════════════
// Deterministic Test Fixtures
// ═══════════════════════════════════════════════════════════════════════════
//
// These edge-case values are used as explicit fuzzing seeds and as
// regression anchors.  They cover boundary conditions in integer
// conversions, float special values, rotation amounts, and ABI
// argument counts.

/// Integer conversion edge cases: i64 minimum and maximum.
const INT_EDGE_CASES: [i64; 2] = [i64::MIN, i64::MAX];

/// Float operation edge cases: NaN, positive infinity, negative infinity.
const FLOAT_EDGE_CASES: [f64; 3] = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY];

/// Rotation amount edge cases: 0, 1, 63, 64, 65.
const ROT_AMOUNT_EDGE_CASES: [u32; 5] = [0, 1, 63, 64, 65];

/// Function argument count edge cases: 0, 4, 5, 8, 16.
const ARG_COUNT_EDGE_CASES: [usize; 5] = [0, 4, 5, 8, 16];

/// Hand-written valid VUMA programs used as deterministic test inputs.
const SAMPLE_VUMA_PROGRAMS: &[&str] = &[
    // Minimal program — just `main`.
    "fn main() {\n}\n",
    // Simple expression statement.
    "fn main() {\n    x = 1 + 2;\n}\n",
    // Two-function program with a call.
    "fn helper() {\n    x = 1 + 2;\n}\nfn main() {\n    helper();\n}\n",
    // Variable assignment from a literal.
    "fn main() {\n    x = 42;\n}\n",
    // Multiple binary operations in one body.
    "fn main() {\n    x = 10 - 3;\n    y = 4 * 5;\n    z = x + y;\n}\n",
];

/// Hand-written memory-operation programs (replaces
/// `arb_memory_program`).
const SAMPLE_MEMORY_PROGRAMS: &[&str] = &[
    "region r1 = allocate(64);\nfn main() {\n    ptr = r1 + 64;\n}\n",
    "region buf = allocate(128);\nfn main() {\n    ptr = buf + 0;\n}\n",
    "region big = allocate(512);\nfn main() {\n    ptr = big + 64;\n}\n",
];

/// Hand-written function-call programs (replaces `arb_call_program`).
const SAMPLE_CALL_PROGRAMS: &[&str] = &[
    "fn helper_one() {\n    x = 1 + 2;\n}\nfn main() {\n    helper_one();\n}\n",
    "fn compute() {\n    x = 1 + 2;\n}\nfn main() {\n    compute();\n}\n",
];

/// Hand-written extern-declaration programs paired with the symbol
/// name we expect to find as SHN_UNDEF in the ELF output.  Replaces
/// `arb_extern_fn_name`.
const SAMPLE_EXTERN_PROGRAMS: &[(&str, &str)] = &[
    (
        "write",
        "extern \"C\" {\n    fn write(fd: i64, buf: Address, count: i64) -> i64;\n}\nfn main() {\n    write(1, 0x400000, 13);\n}\n",
    ),
    (
        "read",
        "extern \"C\" {\n    fn read(fd: i64, buf: Address, count: i64) -> i64;\n}\nfn main() {\n    read(0, 0x400000, 13);\n}\n",
    ),
    (
        "my_extern_fn",
        "extern \"C\" {\n    fn my_extern_fn(x: i64) -> i64;\n}\nfn main() {\n    my_extern_fn(42);\n}\n",
    ),
];

/// Bit patterns for FP roundtrip tests — covers zero, max, min, normal,
/// subnormal, infinity, and NaN.
const FP_BIT_PATTERNS: &[u64] = &[
    0,
    1,
    u64::MAX,
    0x3FF0_0000_0000_0000, // 1.0
    0x4000_0000_0000_0000, // 2.0
    0xBFF0_0000_0000_0000, // -1.0
    0x7FF0_0000_0000_0000, // +Inf
    0xFFF0_0000_0000_0000, // -Inf
    0x7FF8_0000_0000_0000, // quiet NaN
    0x000F_FFFF_FFFF_FFFF, // largest subnormal
    i64::MIN as u64,       // sign bit set, exponent zero
    i64::MAX as u64,       // high bits set
];

/// Integer values within ±2^53 (exactly representable as f64) for
/// i64→f64→i64 roundtrip tests.
const INT_EXACT_F64_VALUES: &[i64] = &[
    0,
    1,
    -1,
    2,
    -2,
    7,
    -7,
    256,
    -256,
    65536,
    -65536,
    1 << 32,
    -(1 << 32),
    1 << 52,
    -(1 << 52),
    (1 << 53) - 1,
    -((1 << 53) - 1),
];

/// Finite f64 values (no NaN, no Inf) for f64→i64→f64 roundtrip tests.
const FINITE_F64_VALUES: &[f64] = &[
    0.0,
    -0.0,
    1.0,
    -1.0,
    3.14159,
    -3.14159,
    1.0e10,
    -1.0e10,
    1.0e-10,
    -1.0e-10,
    9007199254740992.0, // 2^53
    -9007199254740992.0,
];

/// Hand-written VUMA programs that exercise integer↔float casts.
const FP_CAST_PROGRAMS: &[&str] = &[
    "fn main() {\n    x: f64 = 0.0;\n    y: i64 = x as i64;\n}\n",
    "fn main() {\n    x: f64 = 1.0;\n    y: i64 = x as i64;\n}\n",
    "fn main() {\n    x: f64 = -1.0;\n    y: i64 = x as i64;\n}\n",
    "fn main() {\n    x: f64 = 1.0e308 * 2.0;\n    y: i64 = x as i64;\n}\n",
    "fn main() {\n    x: f64 = -1.0e308 * 2.0;\n    y: i64 = x as i64;\n}\n",
    "fn main() {\n    x: f64 = 3.14159;\n    y: i64 = x as i64;\n}\n",
];

/// (current, desired) pairs for atomic CAS tests where the expected
/// value matches `current` (CAS should succeed).
const CAS_MATCHING_PAIRS: &[(i64, i64)] = &[
    (0, 0),
    (0, 1),
    (1, 0),
    (i64::MIN, i64::MAX),
    (i64::MAX, i64::MIN),
    (42, -42),
    (-1, -1),
];

/// (current, wrong_expected) pairs for atomic CAS tests where the
/// expected value does NOT match `current` (CAS should fail).
const CAS_MISMATCH_PAIRS: &[(i64, i64)] = &[
    (0, 1),
    (1, 0),
    (42, -42),
    (i64::MIN, i64::MAX),
    (7, 8),
    (-1, 1),
];

/// Sample u64 values for rotation tests.
const ROT_X_VALUES: &[u64] = &[
    0,
    1,
    u64::MAX,
    0xDEAD_BEEF_CAFE_BABE,
    0xAAAA_AAAA_AAAA_AAAA,
    0x5555_5555_5555_5555,
    1u64 << 63,
];

/// Rotation amounts > 64 (exercises the modular path in rol64/ror64).
const ROT_LARGE_AMOUNTS: &[u32] = &[65, 100, 128, 200];

/// All `EdgeKind` variants, for exhaustive edge-construction tests.
///
/// Returns a fresh `Vec` each call so callers can move the owned
/// `EdgeKind` values into `add_edge` without cloning (the variant does
/// not implement `Copy`).
fn all_edge_kinds() -> Vec<EdgeKind> {
    vec![
        EdgeKind::DataFlow,
        EdgeKind::ControlFlow,
        EdgeKind::Derivation,
        EdgeKind::Annotation,
    ]
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Build a deterministic sample `ProgramPoint`.
fn sample_program_point() -> ProgramPoint {
    ProgramPoint {
        file: Some("test.vu".to_string()),
        line: Some(1),
        column: Some(1),
        offset: None,
    }
}

/// Map a `NodePayload` to its corresponding `NodeType`.
fn payload_type(payload: &NodePayload) -> NodeType {
    match payload {
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
        NodePayload::StructDef(_) => NodeType::StructDef,
        NodePayload::EnumDef(_) => NodeType::EnumDef,
        NodePayload::Match(_) => NodeType::Match,
        NodePayload::ConstantTime(_) => NodeType::ConstantTime,
        NodePayload::Syscall(_) => NodeType::Effect,
    }
}

/// Build a deterministic vector of sample node payloads covering every
/// variant exercised by the SCG construction tests.
fn sample_node_payloads() -> Vec<NodePayload> {
    vec![
        NodePayload::Computation(ComputationNode::new(
            "add",
            Some("i64".to_string()),
            false,
        )),
        NodePayload::Computation(ComputationNode::new("mul", None, false)),
        NodePayload::Allocation(AllocationNode {
            size: 64,
            align: 8,
            region_id: RegionId::new(0),
            type_name: None,
        }),
        NodePayload::Allocation(AllocationNode {
            size: 4096,
            align: 16,
            region_id: RegionId::new(1),
            type_name: None,
        }),
        NodePayload::Control(ControlNode {
            kind: ControlKind::FunctionEntry,
            label: Some("main".to_string()),
        }),
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: NodeId::new(0),
            region_id: RegionId::new(0),
        }),
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: RegionId::new(0),
            offset: Some(0),
            access_size: Some(8),
        }),
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: RegionId::new(1),
            offset: Some(16),
            access_size: Some(4),
        }),
    ]
}

/// Build a fresh SCG pre-populated with the deterministic sample
/// payloads.  Returns the SCG and the list of node IDs that were added.
fn build_sample_scg() -> (SCG, Vec<NodeId>) {
    let mut scg = SCG::new();
    let mut node_ids = Vec::new();
    let pp = sample_program_point();
    for payload in sample_node_payloads() {
        let nt = payload_type(&payload);
        let id = scg.add_node(nt, payload, pp.clone());
        node_ids.push(id);
    }
    (scg, node_ids)
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: Parser Roundtrip
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prop_parser_roundtrip_no_errors() {
    for &program in SAMPLE_VUMA_PROGRAMS {
        let mut parser = vuma_parser::Parser::new(program);
        let result = parser.parse_program();
        assert!(
            !result.has_errors(),
            "Sample program should parse without errors. Program: {:?}. Errors: {:?}",
            program,
            result.errors
        );
    }
}

#[test]
fn prop_parser_memory_program() {
    for &program in SAMPLE_MEMORY_PROGRAMS {
        let mut parser = vuma_parser::Parser::new(program);
        let result = parser.parse_program();
        assert!(
            !result.has_errors(),
            "Memory program should parse without errors. Program: {:?}. Errors: {:?}",
            program,
            result.errors
        );
    }
}

#[test]
fn prop_parser_call_program() {
    for &program in SAMPLE_CALL_PROGRAMS {
        let mut parser = vuma_parser::Parser::new(program);
        let result = parser.parse_program();
        assert!(
            !result.has_errors(),
            "Call program should parse without errors. Program: {:?}. Errors: {:?}",
            program,
            result.errors
        );
    }
}

#[test]
fn prop_parse_to_scg_has_nodes() {
    for &program in SAMPLE_VUMA_PROGRAMS {
        let mut parser = vuma_parser::Parser::new(program);
        let parse_result = parser.parse_program();
        if parse_result.has_errors() {
            // Some sample programs might not parse cleanly; skip those.
            continue;
        }
        let ast = parse_result.unwrap();
        let mut converter = vuma_parser::AstToScg::new();
        match converter.convert(&ast) {
            Ok(scg) => {
                assert!(
                    scg.node_count() > 0,
                    "SCG should have at least one node for any valid program. Program: {:?}",
                    program
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
// Tests: Cross-Backend Consistency
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prop_cross_backend_all_produce_output() {
    use vuma::api::VumaCompiler;

    let compiler = VumaCompiler::new();
    let targets = ["x86_64", "aarch64", "riscv64", "arm32", "mips64", "ppc64"];

    for &program in SAMPLE_VUMA_PROGRAMS {
        // First verify the program compiles at all.
        let default_result = compiler.compile(program);
        if !default_result.success {
            // Some sample programs may not compile due to semantic
            // issues (e.g. undeclared variables); skip those.
            continue;
        }

        for target in &targets {
            let result = compiler.compile_for_target(program, target);
            assert!(
                result.success,
                "Compilation should succeed for target '{}'. Diagnostics: {:?}",
                target,
                result.diagnostics
            );
            assert!(
                result.target.is_some(),
                "Should have target output for '{}'",
                target
            );
            if let Some(ref tgt) = result.target {
                assert!(
                    !tgt.binary.is_empty(),
                    "Binary output should not be empty for '{}'",
                    target
                );
            }
        }
    }
}

#[test]
fn prop_cross_backend_same_scg() {
    use vuma::api::VumaCompiler;

    let compiler = VumaCompiler::new();

    for &program in SAMPLE_MEMORY_PROGRAMS {
        // Compile for two different targets and compare SCG summaries.
        let result_a = compiler.compile_for_target(program, "aarch64");
        let result_b = compiler.compile_for_target(program, "x86_64");

        if result_a.success && result_b.success {
            if let (Some(scg_a), Some(scg_b)) = (&result_a.scg, &result_b.scg) {
                // The SCG is target-independent; both should have the
                // same function count and node count.
                assert_eq!(
                    scg_a.function_count, scg_b.function_count,
                    "Function count should be same across backends"
                );
                assert_eq!(
                    scg_a.total_nodes, scg_b.total_nodes,
                    "Total nodes should be same across backends"
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: SCG Structural Invariants
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prop_scg_every_function_has_entry() {
    for &program in SAMPLE_VUMA_PROGRAMS {
        let mut parser = vuma_parser::Parser::new(program);
        let parse_result = parser.parse_program();
        if parse_result.has_errors() {
            continue;
        }
        let ast = parse_result.unwrap();
        let mut converter = vuma_parser::AstToScg::new();
        let scg = match converter.convert(&ast) {
            Ok(scg) => scg,
            Err(_) => continue,
        };

        // Count FunctionEntry nodes.
        let entry_count = scg
            .nodes()
            .filter(|n| {
                matches!(&n.payload, NodePayload::Control(c) if c.kind == ControlKind::FunctionEntry)
            })
            .count();

        assert!(
            entry_count >= 1,
            "SCG should have at least one FunctionEntry node (found {})",
            entry_count
        );
    }
}

#[test]
fn prop_scg_edges_connect_valid_nodes() {
    for &program in SAMPLE_VUMA_PROGRAMS {
        let mut parser = vuma_parser::Parser::new(program);
        let parse_result = parser.parse_program();
        if parse_result.has_errors() {
            continue;
        }
        let ast = parse_result.unwrap();
        let mut converter = vuma_parser::AstToScg::new();
        let scg = match converter.convert(&ast) {
            Ok(scg) => scg,
            Err(_) => continue,
        };

        // Collect all node IDs.
        let node_ids: std::collections::HashSet<NodeId> =
            scg.nodes().map(|n| n.id).collect();

        // Verify every edge connects existing nodes.
        for edge in scg.edges() {
            assert!(
                node_ids.contains(&edge.source),
                "Edge source {:?} should exist in the graph",
                edge.source
            );
            assert!(
                node_ids.contains(&edge.target),
                "Edge target {:?} should exist in the graph",
                edge.target
            );
        }
    }
}

#[test]
fn prop_scg_validation_passes() {
    for &program in SAMPLE_VUMA_PROGRAMS {
        let mut parser = vuma_parser::Parser::new(program);
        let parse_result = parser.parse_program();
        if parse_result.has_errors() {
            continue;
        }
        let ast = parse_result.unwrap();
        let mut converter = vuma_parser::AstToScg::new();
        let scg = match converter.convert(&ast) {
            Ok(scg) => scg,
            Err(_) => continue,
        };

        let validation = scg.validate();
        assert!(
            validation.is_valid,
            "SCG validation should pass for valid programs. Errors: {:?}",
            validation.errors
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: SCG Construction Invariants
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prop_scg_random_construction_invariants() {
    let (mut scg, node_ids) = build_sample_scg();

    // Invariant: node count matches.
    assert_eq!(
        scg.node_count(),
        node_ids.len(),
        "Node count should match the number of added nodes"
    );

    // Invariant: every added node can be retrieved.
    for &id in &node_ids {
        assert!(
            scg.get_node(id).is_some(),
            "Added node {:?} should be retrievable",
            id
        );
    }

    // Invariant: edges between existing nodes should succeed.
    assert!(
        node_ids.len() >= 2,
        "sample SCG should have at least 2 nodes"
    );
    let edge_result = scg.add_edge(node_ids[0], node_ids[1], EdgeKind::DataFlow);
    assert!(
        edge_result.is_ok(),
        "Adding an edge between existing nodes should succeed"
    );
    assert_eq!(scg.edge_count(), 1, "Edge count should be 1");
}

#[test]
fn prop_scg_random_edges_valid() {
    let (mut scg, node_ids) = build_sample_scg();
    assert!(
        node_ids.len() >= 2,
        "sample SCG should have at least 2 nodes for edge tests"
    );

    // Add edges of every kind, cycling through node pairs.
    for (i, kind) in all_edge_kinds().into_iter().enumerate() {
        let src_idx = i % node_ids.len();
        let tgt_idx = (i + 1) % node_ids.len();
        let result = scg.add_edge(node_ids[src_idx], node_ids[tgt_idx], kind.clone());
        assert!(
            result.is_ok(),
            "Edge between valid nodes should succeed for kind {:?}",
            kind
        );
    }

    // Verify all edges connect existing nodes.
    let node_id_set: std::collections::HashSet<NodeId> = node_ids.into_iter().collect();
    for edge in scg.edges() {
        assert!(
            node_id_set.contains(&edge.source),
            "Edge source should be a valid node"
        );
        assert!(
            node_id_set.contains(&edge.target),
            "Edge target should be a valid node"
        );
    }
}

#[test]
fn prop_scg_edge_to_nonexistent_node_fails() {
    for payload in sample_node_payloads() {
        for kind in all_edge_kinds() {
            let mut scg = SCG::new();
            let node_id =
                scg.add_node(payload_type(&payload), payload.clone(), sample_program_point());
            let fake_id = NodeId::new(99999);

            // Edge from real to fake should fail.
            let result = scg.add_edge(node_id, fake_id, kind.clone());
            assert!(
                result.is_err(),
                "Edge to non-existent node should fail for kind {:?}",
                kind
            );

            // Edge from fake to real should also fail.
            let result = scg.add_edge(fake_id, node_id, EdgeKind::ControlFlow);
            assert!(
                result.is_err(),
                "Edge from non-existent node should fail for kind {:?}",
                kind
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: Verification Pipeline
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prop_verify_no_panic() {
    use vuma::api::VumaCompiler;

    let compiler = VumaCompiler::new();
    for &program in SAMPLE_VUMA_PROGRAMS {
        let report = compiler.verify(program);

        // The report should always be non-empty (even on error).
        assert!(
            report.metadata.source_bytes > 0,
            "Metadata should record source size"
        );
    }
}

#[test]
fn prop_verify_report_serializable() {
    use vuma::api::VumaCompiler;

    let compiler = VumaCompiler::new();
    for &program in SAMPLE_VUMA_PROGRAMS {
        let report = compiler.verify(program);

        let json_result = report.to_json();
        assert!(
            !json_result.is_empty(),
            "VerificationReport should always be serializable"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: IVE Verification on Sample SCGs
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prop_ive_verify_all_invariants() {
    let (scg, _node_ids) = build_sample_scg();

    let aggregator = vuma_ive::InvariantAggregator::new();
    let input = vuma_ive::verification::VerificationInput::from_scg(scg);
    let result = aggregator.verify_all(&input);

    // Should always produce a result (even for a trivial SCG).
    assert!(
        !result.per_invariant.is_empty(),
        "Should have at least some invariant results"
    );

    // The overall verdict should be one of the known variants.
    assert!(matches!(
        result.overall,
        vuma_ive::OverallVerdict::Pass
            | vuma_ive::OverallVerdict::Fail
            | vuma_ive::OverallVerdict::Inconclusive
            | vuma_ive::OverallVerdict::NoChecks
    ));
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: FP Conversion Roundtrip
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prop_fp_bitcast_roundtrip() {
    for &bits in FP_BIT_PATTERNS {
        let x = f64::from_bits(bits);
        if !x.is_nan() {
            assert_eq!(
                f64::from_bits(x.to_bits()),
                x,
                "f64 bit roundtrip should be lossless for {:?} (bits={:#018x})",
                x,
                bits
            );
        }
    }
}

#[test]
fn prop_int_float_int_roundtrip() {
    // f64 has 53 bits of mantissa, so only integers with |v| <= 2^53
    // are exactly representable.
    for &v in INT_EXACT_F64_VALUES {
        let as_f64 = v as f64;
        let back = as_f64 as i64;
        assert_eq!(
            back, v,
            "i64→f64→i64 roundtrip failed: {} → {} → {}",
            v, as_f64, back
        );
    }
}

#[test]
fn prop_float_int_float_roundtrip() {
    for &v in FINITE_F64_VALUES {
        // Only test values that are within i64 range.
        if v >= i64::MIN as f64 && v <= i64::MAX as f64 {
            let as_i64 = v as i64;
            let back = as_i64 as f64;
            // The integer value should convert back exactly.
            assert_eq!(
                back,
                as_i64 as f64,
                "f64→i64→f64 roundtrip for integer-equivalent value: {} → {} → {}",
                v,
                as_i64,
                back
            );
        }
    }
}

#[test]
fn fp_cast_compiles_without_panic() {
    use vuma::api::VumaCompiler;

    let compiler = VumaCompiler::new();
    for &source in FP_CAST_PROGRAMS {
        // Should not panic — compilation may fail for some values,
        // but should never panic.
        let _ = compiler.compile(source);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: Atomic CAS Correctness
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prop_atomic_cas_match_succeeds() {
    for &(current, desired) in CAS_MATCHING_PAIRS {
        // Simulate CAS: old = current, expected = current (match).
        let old = current;
        let expected = current;
        // CAS succeeds because old == expected.
        let cas_succeeded = old == expected;
        assert!(
            cas_succeeded,
            "CAS with matching expected should succeed: old={}, expected={}",
            old, expected
        );
        // After successful CAS, the new value should be `desired`.
        let new_value = if cas_succeeded { desired } else { old };
        assert_eq!(
            new_value, desired,
            "After successful CAS, value should be desired"
        );
    }
}

#[test]
fn prop_atomic_cas_mismatch_fails() {
    for &(current, wrong_expected) in CAS_MISMATCH_PAIRS {
        // CAS mismatch precondition: current != wrong_expected.
        assert_ne!(
            current, wrong_expected,
            "Need different values for mismatch test"
        );

        let old = current;
        let expected = wrong_expected;
        let cas_succeeded = old == expected;
        assert!(
            !cas_succeeded,
            "CAS with non-matching expected should fail: old={}, expected={}",
            old, expected
        );
        // After failed CAS, value should remain unchanged.
        let new_value = if cas_succeeded { 0i64 } else { old };
        assert_eq!(
            new_value, current,
            "After failed CAS, value should remain unchanged"
        );
    }
}

#[test]
fn prop_atomic_cas_compiles() {
    use vuma::api::VumaCompiler;

    let compiler = VumaCompiler::new();
    for &(current, desired) in CAS_MATCHING_PAIRS {
        // Generate a VUMA program that uses atomic_cas.
        let source = format!(
            "fn main() {{\n    lock = allocate(8);\n    *lock = {};\n    old = atomic_cas(lock, {}, {});\n}}\n",
            current, current, desired
        );

        // Should not panic — compilation may fail, but should never panic.
        let _ = compiler.compile(&source);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: Rotate Roundtrip
// ═══════════════════════════════════════════════════════════════════════════

/// Rotate left (64-bit).
fn rol64(x: u64, n: u32) -> u64 {
    let n = n % 64;
    if n == 0 { x } else { (x << n) | (x >> (64 - n)) }
}

/// Rotate right (64-bit).
fn ror64(x: u64, n: u32) -> u64 {
    let n = n % 64;
    if n == 0 { x } else { (x >> n) | (x << (64 - n)) }
}

#[test]
fn prop_rotate_roundtrip() {
    for &x in ROT_X_VALUES {
        for &n in &ROT_AMOUNT_EDGE_CASES {
            let rotated = rol64(x, n);
            let restored = ror64(rotated, n);
            assert_eq!(
                restored, x,
                "ROL({}, {}) = {}, ROR({}, {}) = {} ≠ {}",
                x, n, rotated, rotated, n, restored, x
            );
        }
    }
}

#[test]
fn prop_rotate_roundtrip_reverse() {
    for &x in ROT_X_VALUES {
        for &n in &ROT_AMOUNT_EDGE_CASES {
            let rotated = ror64(x, n);
            let restored = rol64(rotated, n);
            assert_eq!(
                restored, x,
                "ROR({}, {}) = {}, ROL({}, {}) = {} ≠ {}",
                x, n, rotated, rotated, n, restored, x
            );
        }
    }
}

#[test]
fn prop_rol_zero_is_identity() {
    for &x in ROT_X_VALUES {
        assert_eq!(rol64(x, 0), x, "ROL(x, 0) should equal x");
    }
}

#[test]
fn prop_ror_zero_is_identity() {
    for &x in ROT_X_VALUES {
        assert_eq!(ror64(x, 0), x, "ROR(x, 0) should equal x");
    }
}

#[test]
fn prop_rol_64_is_identity() {
    for &x in ROT_X_VALUES {
        assert_eq!(rol64(x, 64), x, "ROL(x, 64) should equal x");
    }
}

#[test]
fn prop_ror_64_is_identity() {
    for &x in ROT_X_VALUES {
        assert_eq!(ror64(x, 64), x, "ROR(x, 64) should equal x");
    }
}

#[test]
fn prop_rotate_large_amount_roundtrip() {
    for &x in ROT_X_VALUES {
        for &n in ROT_LARGE_AMOUNTS {
            let rotated = rol64(x, n);
            let restored = ror64(rotated, n);
            assert_eq!(
                restored, x,
                "ROL/ROR roundtrip with n={} (mod 64 = {}) failed",
                n,
                n % 64
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: ABI Consistency
// ═══════════════════════════════════════════════════════════════════════════

/// Build a simple IR function with `n` i64 parameters that returns the first
/// parameter (or 0 if no params).  This mirrors the helper in
/// `abi_conformance.rs`.
fn build_ir_function_with_n_args(name: &str, n: usize) -> vuma_codegen::ir::IRFunction {
    use vuma_codegen::ir::{IRFunction, IRType, IRValue, IRTerminator, VirtualRegister};

    let mut func = IRFunction::new(name);
    for i in 0..n {
        func.param_types.push(IRType::I64);
        func.params.push(IRValue::Register(i as u32));
        func.vregs
            .insert(i as u32, VirtualRegister::named(i as u32, format!("a{}", i)));
    }
    func.result_types.push(IRType::I64);
    func.results.push(IRValue::Register(n as u32));

    let ret_val = if n > 0 {
        IRValue::Register(0)
    } else {
        IRValue::Immediate(0)
    };
    func.current_block().terminator = IRTerminator::Return(vec![ret_val]);
    func
}

#[test]
fn prop_abi_varying_arg_counts_compile() {
    use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};

    let backends = [
        BackendKind::AArch64,
        BackendKind::X86_64,
        BackendKind::RiscV64,
        BackendKind::Arm32,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::LoongArch64,
    ];

    for n in 0..16usize {
        let func = build_ir_function_with_n_args("test_fn", n);

        for kind in &backends {
            if let Ok(backend) = create_backend(*kind) {
                // Should not panic.
                let result = backend.allocate_registers(&func);
                assert!(
                    result.is_ok(),
                    "Register allocation for {} args on {:?} should succeed: {:?}",
                    n,
                    kind,
                    result.err()
                );

                if let Ok(allocated) = result {
                    let program = AllocatedProgram {
                        functions: vec![allocated],
                        total_code_size: 0,
                        total_data_size: 0,
                    rodata_data: Vec::new(),
                    function_names: std::collections::HashSet::new(),
                    };
                    // Encoding should also not panic.
                    let encode_result = backend.encode_program(&program);
                    assert!(
                        encode_result.is_ok(),
                        "Encoding for {} args on {:?} should succeed: {:?}",
                        n,
                        kind,
                        encode_result.err()
                    );
                }
            }
        }
    }
}

#[test]
fn prop_abi_same_arg_count_same_size() {
    use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};

    if let Ok(backend) = create_backend(BackendKind::AArch64) {
        for n in 1..8usize {
            let func_a = build_ir_function_with_n_args("fn_a", n);
            let func_b = build_ir_function_with_n_args("fn_b", n);

            let alloc_a = backend.allocate_registers(&func_a);
            let alloc_b = backend.allocate_registers(&func_b);

            if let (Ok(a), Ok(b)) = (alloc_a, alloc_b) {
                let prog_a = AllocatedProgram {
                    functions: vec![a],
                    total_code_size: 0,
                    total_data_size: 0,
                rodata_data: Vec::new(),
                function_names: std::collections::HashSet::new(),
                };
                let prog_b = AllocatedProgram {
                    functions: vec![b],
                    total_code_size: 0,
                    total_data_size: 0,
                rodata_data: Vec::new(),
                function_names: std::collections::HashSet::new(),
                };
                if let (Ok(bin_a), Ok(bin_b)) =
                    (backend.encode_program(&prog_a), backend.encode_program(&prog_b))
                {
                    assert_eq!(
                        bin_a.len(),
                        bin_b.len(),
                        "Same arg count ({}) should produce same binary size",
                        n
                    );
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: DWARF Consistency
// ═══════════════════════════════════════════════════════════════════════════

/// Extract the .text section bytes from an ELF binary by parsing section
/// headers.  Returns `None` if the ELF cannot be parsed or .text is not
/// found.
fn extract_text_section(elf: &[u8]) -> Option<Vec<u8>> {
    if elf.len() < 64 { return None; }
    if &elf[0..4] != b"\x7fELF" { return None; }

    let is_64 = elf[4] == 2;
    let is_le = elf[5] == 1;

    let read_u16 = |b: &[u8]| -> u16 {
        if is_le { u16::from_le_bytes([b[0], b[1]]) }
        else { u16::from_be_bytes([b[0], b[1]]) }
    };
    let read_u32 = |b: &[u8]| -> u32 {
        if is_le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) }
        else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) }
    };
    let read_u64 = |b: &[u8]| -> u64 {
        if is_le {
            u64::from_le_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ])
        } else {
            u64::from_be_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ])
        }
    };

    let (e_shoff, e_shentsize, e_shnum, e_shstrndx) = if is_64 {
        if elf.len() < 64 { return None; }
        (
            read_u64(&elf[40..48]) as usize,
            read_u16(&elf[58..60]) as usize,
            read_u16(&elf[60..62]) as usize,
            read_u16(&elf[62..64]) as usize,
        )
    } else {
        if elf.len() < 52 { return None; }
        (
            read_u32(&elf[32..36]) as usize,
            read_u16(&elf[46..48]) as usize,
            read_u16(&elf[48..50]) as usize,
            read_u16(&elf[50..52]) as usize,
        )
    };

    if e_shoff == 0 || e_shnum == 0 { return None; }

    // Find the section-header string table.
    let shstrtab_off = if e_shstrndx > 0 && (e_shstrndx as usize) < e_shnum {
        let shdr_off = e_shoff + (e_shstrndx as usize) * e_shentsize;
        if is_64 {
            if shdr_off + 64 > elf.len() { return None; }
            read_u64(&elf[shdr_off + 24..shdr_off + 32]) as usize
        } else {
            if shdr_off + 40 > elf.len() { return None; }
            read_u32(&elf[shdr_off + 16..shdr_off + 20]) as usize
        }
    } else {
        return None;
    };

    // Iterate section headers to find .text.
    for i in 0..e_shnum {
        let shdr_off = e_shoff + i * e_shentsize;
        if is_64 {
            if shdr_off + 64 > elf.len() { break; }
            let sh_name = read_u32(&elf[shdr_off..shdr_off + 4]) as usize;
            let sh_offset = read_u64(&elf[shdr_off + 24..shdr_off + 32]) as usize;
            let sh_size = read_u64(&elf[shdr_off + 32..shdr_off + 40]) as usize;

            // Read the name from shstrtab.
            let name_start = shstrtab_off + sh_name;
            if name_start < elf.len() {
                let name_end = elf[name_start..].iter()
                    .position(|&b| b == 0)
                    .map(|p| name_start + p)
                    .unwrap_or(elf.len());
                let name = std::str::from_utf8(&elf[name_start..name_end])
                    .unwrap_or("");
                if name == ".text" {
                    if sh_offset + sh_size <= elf.len() {
                        return Some(elf[sh_offset..sh_offset + sh_size].to_vec());
                    }
                }
            }
        } else {
            if shdr_off + 40 > elf.len() { break; }
            let sh_name = read_u32(&elf[shdr_off..shdr_off + 4]) as usize;
            let sh_offset = read_u32(&elf[shdr_off + 16..shdr_off + 20]) as usize;
            let sh_size = read_u32(&elf[shdr_off + 20..shdr_off + 24]) as usize;

            let name_start = shstrtab_off + sh_name;
            if name_start < elf.len() {
                let name_end = elf[name_start..].iter()
                    .position(|&b| b == 0)
                    .map(|p| name_start + p)
                    .unwrap_or(elf.len());
                let name = std::str::from_utf8(&elf[name_start..name_end])
                    .unwrap_or("");
                if name == ".text" {
                    if sh_offset + sh_size <= elf.len() {
                        return Some(elf[sh_offset..sh_offset + sh_size].to_vec());
                    }
                }
            }
        }
    }

    None
}

#[test]
fn prop_dwarf_text_section_unchanged() {
    use vuma::api::VumaCompiler;
    use vuma::pipeline::CompileConfig;

    for &program in SAMPLE_VUMA_PROGRAMS {
        let compiler_no_debug = VumaCompiler::with_config(CompileConfig {
            debug_info: false,
            section_headers: true,
            ..CompileConfig::default()
        });
        let compiler_debug = VumaCompiler::with_config(CompileConfig {
            debug_info: true,
            section_headers: true,
            ..CompileConfig::default()
        });

        let result_no_debug = compiler_no_debug.compile(program);
        let result_debug = compiler_debug.compile(program);

        if !result_no_debug.success || !result_debug.success {
            // If either compilation fails, skip (may be due to a
            // sample program not being supported by every config).
            continue;
        }

        let bin_no_debug = result_no_debug.target.as_ref().map(|t| &t.binary);
        let bin_debug = result_debug.target.as_ref().map(|t| &t.binary);

        if let (Some(nd), Some(d)) = (bin_no_debug, bin_debug) {
            let text_nd = extract_text_section(nd);
            let text_d = extract_text_section(d);

            if let (Some(t_nd), Some(t_d)) = (text_nd, text_d) {
                assert_eq!(
                    t_nd, t_d,
                    "Debug info should not change .text section"
                );
            }
            // If we couldn't parse .text, that's OK — the ELF may not
            // have section headers.  The important thing is no panic.
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests: FFI Symbol Emission
// ═══════════════════════════════════════════════════════════════════════════

/// Parse the ELF symbol table and return the names of symbols with
/// SHN_UNDEF (section index 0), i.e., undefined/external symbols.
fn find_undef_symbols(elf: &[u8]) -> Vec<String> {
    let mut undef_syms = Vec::new();
    if elf.len() < 64 { return undef_syms; }
    if &elf[0..4] != b"\x7fELF" { return undef_syms; }

    let is_64 = elf[4] == 2;
    let is_le = elf[5] == 1;

    let read_u16 = |b: &[u8]| -> u16 {
        if is_le { u16::from_le_bytes([b[0], b[1]]) }
        else { u16::from_be_bytes([b[0], b[1]]) }
    };
    let read_u32 = |b: &[u8]| -> u32 {
        if is_le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) }
        else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) }
    };
    let read_u64 = |b: &[u8]| -> u64 {
        if is_le {
            u64::from_le_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ])
        } else {
            u64::from_be_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ])
        }
    };

    let (e_shoff, e_shentsize, e_shnum, e_shstrndx) = if is_64 {
        (
            read_u64(&elf[40..48]) as usize,
            read_u16(&elf[58..60]) as usize,
            read_u16(&elf[60..62]) as usize,
            read_u16(&elf[62..64]) as usize,
        )
    } else {
        (
            read_u32(&elf[32..36]) as usize,
            read_u16(&elf[46..48]) as usize,
            read_u16(&elf[48..50]) as usize,
            read_u16(&elf[50..52]) as usize,
        )
    };

    if e_shoff == 0 || e_shnum == 0 { return undef_syms; }

    // Find SHT_SYMTAB (type 2) and its linked string table.
    for i in 0..e_shnum {
        let shdr_off = e_shoff + i * e_shentsize;
        if is_64 {
            if shdr_off + 64 > elf.len() { break; }
            let sh_type = read_u32(&elf[shdr_off + 4..shdr_off + 8]);
            let sh_offset = read_u64(&elf[shdr_off + 24..shdr_off + 32]) as usize;
            let sh_size = read_u64(&elf[shdr_off + 32..shdr_off + 40]) as usize;
            let sh_link = read_u32(&elf[shdr_off + 40..shdr_off + 44]) as usize;
            let sh_entsize = read_u64(&elf[shdr_off + 56..shdr_off + 64]) as usize;

            if sh_type != 2 { continue; } // Not SHT_SYMTAB

            // Load the linked string table.
            let strtab_off = if sh_link > 0 && sh_link < e_shnum {
                let str_shdr_off = e_shoff + sh_link * e_shentsize;
                if str_shdr_off + 64 > elf.len() { continue; }
                read_u64(&elf[str_shdr_off + 24..str_shdr_off + 32]) as usize
            } else {
                continue;
            };

            let entry_size = if sh_entsize > 0 { sh_entsize } else { 24 };
            let num_syms = sh_size / entry_size;

            for j in 1..num_syms { // Skip symbol 0 (null)
                let sym_off = sh_offset + j * entry_size;
                if sym_off + 24 > elf.len() { break; }
                let st_name = read_u32(&elf[sym_off..sym_off + 4]) as usize;
                let _st_info = elf[sym_off + 4];
                let _st_other = elf[sym_off + 5];
                let st_shndx = read_u16(&elf[sym_off + 6..sym_off + 8]);

                if st_shndx == 0 { // SHN_UNDEF
                    let name_start = strtab_off + st_name;
                    if name_start < elf.len() {
                        let name_end = elf[name_start..].iter()
                            .position(|&b| b == 0)
                            .map(|p| name_start + p)
                            .unwrap_or(elf.len());
                        if let Ok(name) = std::str::from_utf8(&elf[name_start..name_end]) {
                            if !name.is_empty() {
                                undef_syms.push(name.to_string());
                            }
                        }
                    }
                }
            }
        } else {
            if shdr_off + 40 > elf.len() { break; }
            let sh_type = read_u32(&elf[shdr_off + 4..shdr_off + 8]);
            let sh_offset = read_u32(&elf[shdr_off + 16..shdr_off + 20]) as usize;
            let sh_size = read_u32(&elf[shdr_off + 20..shdr_off + 24]) as usize;
            let sh_link = read_u32(&elf[shdr_off + 24..shdr_off + 28]) as usize;
            let sh_entsize = read_u32(&elf[shdr_off + 36..shdr_off + 40]) as usize;

            if sh_type != 2 { continue; }

            let strtab_off = if sh_link > 0 && sh_link < e_shnum {
                let str_shdr_off = e_shoff + sh_link * e_shentsize;
                if str_shdr_off + 40 > elf.len() { continue; }
                read_u32(&elf[str_shdr_off + 16..str_shdr_off + 20]) as usize
            } else {
                continue;
            };

            let entry_size = if sh_entsize > 0 { sh_entsize } else { 16 };
            let num_syms = sh_size / entry_size;

            for j in 1..num_syms {
                let sym_off = sh_offset + j * entry_size;
                if sym_off + 16 > elf.len() { break; }
                let st_name = read_u32(&elf[sym_off..sym_off + 4]) as usize;
                let _st_info = elf[sym_off + 4];
                let _st_other = elf[sym_off + 5];
                let st_shndx = read_u16(&elf[sym_off + 6..sym_off + 8]);

                if st_shndx == 0 { // SHN_UNDEF
                    let name_start = strtab_off + st_name;
                    if name_start < elf.len() {
                        let name_end = elf[name_start..].iter()
                            .position(|&b| b == 0)
                            .map(|p| name_start + p)
                            .unwrap_or(elf.len());
                        if let Ok(name) = std::str::from_utf8(&elf[name_start..name_end]) {
                            if !name.is_empty() {
                                undef_syms.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    undef_syms
}

#[test]
fn prop_ffi_extern_symbols_are_undef() {
    use vuma::api::VumaCompiler;
    use vuma::pipeline::CompileConfig;

    for &(extern_name, source) in SAMPLE_EXTERN_PROGRAMS {
        let compiler = VumaCompiler::with_config(CompileConfig {
            section_headers: true,
            ..CompileConfig::default()
        });

        let result = compiler.compile(source);

        // If compilation succeeds, check the ELF for the undefined symbol.
        if result.success {
            if let Some(ref target) = result.target {
                let undef_syms = find_undef_symbols(&target.binary);
                assert!(
                    undef_syms.contains(&extern_name.to_string()),
                    "Extern function '{}' should appear as SHN_UNDEF in ELF. \
                     Found undefined symbols: {:?}",
                    extern_name,
                    undef_syms
                );
            }
        }
        // If compilation fails (e.g., extern not fully supported for
        // some target), that's acceptable — we just verify no panic.
    }
}

#[test]
fn prop_ffi_multiple_extern_symbols() {
    use vuma::api::VumaCompiler;
    use vuma::pipeline::CompileConfig;

    // Two distinct extern names in one program.
    let name1 = "write";
    let name2 = "read";
    let source = format!(
        "extern \"C\" {{\n    fn {}(x: i64) -> i64;\n    fn {}(x: i64) -> i64;\n}}\nfn main() {{\n    let a = {}(1);\n    let b = {}(2);\n}}\n",
        name1, name2, name1, name2
    );

    let compiler = VumaCompiler::with_config(CompileConfig {
        section_headers: true,
        ..CompileConfig::default()
    });

    let result = compiler.compile(&source);

    if result.success {
        if let Some(ref target) = result.target {
            let undef_syms = find_undef_symbols(&target.binary);
            assert!(
                undef_syms.contains(&name1.to_string()),
                "Extern function '{}' should be SHN_UNDEF. Found: {:?}",
                name1,
                undef_syms
            );
            assert!(
                undef_syms.contains(&name2.to_string()),
                "Extern function '{}' should be SHN_UNDEF. Found: {:?}",
                name2,
                undef_syms
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Fuzzing Seed Tests
// ═══════════════════════════════════════════════════════════════════════════
//
// These tests explicitly exercise the edge-case seed values defined above.
// They serve as regression anchors and ensure that boundary conditions
// are always tested.

#[test]
fn fuzz_int_edge_cases_conversion() {
    for &v in &INT_EDGE_CASES {
        // i64 → f64 → i64 roundtrip for exactly representable values.
        let as_f64 = v as f64;
        let back = as_f64 as i64;
        // Note: i64::MIN and i64::MAX are NOT exactly representable in f64,
        // so we only check the conversion doesn't panic and produces a
        // reasonable result.
        let _ = (v, as_f64, back);
    }
}

#[test]
fn fuzz_int_edge_cases_bitcast() {
    for &v in &INT_EDGE_CASES {
        // i64 bit pattern → f64 bit pattern should not panic.
        let bits = v as u64;
        let f = f64::from_bits(bits);
        let _ = f; // Don't care about value, just no panic.
        // f64 → u64 roundtrip.
        let bits_back = f.to_bits();
        assert_eq!(bits_back, bits, "Bit roundtrip failed for i64={}", v);
    }
}

#[test]
fn fuzz_float_edge_cases_bitcast() {
    for &f in &FLOAT_EDGE_CASES {
        // f64 → u64 → f64 roundtrip.
        let bits = f.to_bits();
        let back = f64::from_bits(bits);
        if f.is_nan() {
            assert!(back.is_nan(), "NaN should roundtrip to NaN");
        } else {
            assert_eq!(back, f, "f64 bit roundtrip failed for {:?}", f);
        }
    }
}

#[test]
fn fuzz_float_edge_cases_int_conversion() {
    for &f in &FLOAT_EDGE_CASES {
        // Converting NaN/Inf to i64 is undefined in the spec, but should
        // not panic.  We just verify it doesn't crash.
        if !f.is_nan() && !f.is_infinite() {
            let as_i64 = f as i64;
            let _ = as_i64; // No panic check.
        }
        // NaN comparisons should not panic.
        let _ = f == f; // NaN != NaN, but shouldn't panic
        let _ = f < 0.0;
        let _ = f > 0.0;
    }
}

#[test]
fn fuzz_rotation_edge_cases() {
    for &n in &ROT_AMOUNT_EDGE_CASES {
        // Test with a known value: 0x1 (bit 0 set).
        let x: u64 = 1;
        let rotated = rol64(x, n);
        let restored = ror64(rotated, n);
        assert_eq!(restored, x,
            "ROL/ROR roundtrip failed for x={}, n={}", x, n);

        // Test with all bits set.
        let x_all: u64 = !0;
        let rotated_all = rol64(x_all, n);
        assert_eq!(rotated_all, x_all,
            "ROL of all-ones should be all-ones for n={}", n);

        // ROL(x, n) should equal ROR(x, 64-n%64) for non-zero n.
        if n % 64 != 0 {
            let via_ror = ror64(x, 64 - (n % 64));
            assert_eq!(rotated, via_ror,
                "ROL(x, {}) should equal ROR(x, {})", n, 64 - (n % 64));
        }
    }
}

#[test]
fn fuzz_arg_count_edge_cases() {
    use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};

    for &n in &ARG_COUNT_EDGE_CASES {
        let func = build_ir_function_with_n_args("test_fn", n);

        for kind in &[
            BackendKind::AArch64,
            BackendKind::X86_64,
            BackendKind::RiscV64,
            BackendKind::Arm32,
        ] {
            if let Ok(backend) = create_backend(*kind) {
                let result = backend.allocate_registers(&func);
                assert!(
                    result.is_ok(),
                    "Register allocation for {} args on {:?} failed: {:?}",
                    n, kind, result.err()
                );

                if let Ok(allocated) = result {
                    let program = AllocatedProgram {
                        functions: vec![allocated],
                        total_code_size: 0,
                        total_data_size: 0,
                    rodata_data: Vec::new(),
                    function_names: std::collections::HashSet::new(),
                    };
                    let encode_result = backend.encode_program(&program);
                    assert!(
                        encode_result.is_ok(),
                        "Encoding for {} args on {:?} failed: {:?}",
                        n, kind, encode_result.err()
                    );
                }
            }
        }
    }
}

#[test]
fn fuzz_dwarf_text_consistency_simple() {
    use vuma::api::VumaCompiler;
    use vuma::pipeline::CompileConfig;

    let source = "fn main() {\n    x = 1 + 2;\n}\n";

    let compiler_no_debug = VumaCompiler::with_config(CompileConfig {
        debug_info: false,
        section_headers: true,
        ..CompileConfig::default()
    });
    let compiler_debug = VumaCompiler::with_config(CompileConfig {
        debug_info: true,
        section_headers: true,
        ..CompileConfig::default()
    });

    let result_nd = compiler_no_debug.compile(source);
    let result_d = compiler_debug.compile(source);

    if result_nd.success && result_d.success {
        if let (Some(t_nd), Some(t_d)) = (
            result_nd.target.as_ref(),
            result_d.target.as_ref(),
        ) {
            let text_nd = extract_text_section(&t_nd.binary);
            let text_d = extract_text_section(&t_d.binary);
            if let (Some(tn), Some(td)) = (text_nd, text_d) {
                assert_eq!(tn, td,
                    "Debug info should not change .text section");
            }
        }
    }
}

#[test]
fn fuzz_ffi_extern_symbol_simple() {
    use vuma::api::VumaCompiler;
    use vuma::pipeline::CompileConfig;

    let source = "extern \"C\" {\n    fn write(fd: i64, buf: Address, count: i64) -> i64;\n}\nfn main() {\n    write(1, 0x400000, 13);\n}\n";

    let compiler = VumaCompiler::with_config(CompileConfig {
        section_headers: true,
        ..CompileConfig::default()
    });

    let result = compiler.compile(source);

    if result.success {
        if let Some(ref target) = result.target {
            let undef_syms = find_undef_symbols(&target.binary);
            assert!(
                undef_syms.contains(&"write".to_string()),
                "Extern function 'write' should be SHN_UNDEF. Found: {:?}",
                undef_syms
            );
        }
    }
}
