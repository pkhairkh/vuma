//! Parser Roundtrip Tests
//!
//! Verifies that VUMA source code parses correctly and produces valid
//! AST / SCG intermediate representations. Each test parses VUMA source
//! through the full pipeline (Lexer → Parser → AST → SCG) and checks
//! that the resulting structures are well-formed.
//!
//! # Test Matrix
//!
//! | #  | Test                          | What it validates                            |
//! |----|-------------------------------|----------------------------------------------|
//! | 1  | Minimal program               | Simplest fn main with return                 |
//! | 2  | Function with params          | Typed parameters, binary expression          |
//! | 3  | Memory operations             | allocate, deref, free                        |
//! | 4  | For loop                      | Range-based for iteration                    |
//! | 5  | Nested function calls         | Calls within calls                           |
//! | 6  | u32 masking                   | (x + y) & 4294967295                         |
//! | 7  | Bitwise ops                   | AND, OR, XOR, shifts                        |
//! | 8  | Pointer arithmetic            | *(buf + offset)                              |
//! | 9  | SHA256d parse                 | Full sha256d.vuma example                    |
//! | 10 | Error recovery                | Malformed source → diagnostics, no panic     |

use vuma_parser::{parser::Parser, to_scg::AstToScg, Program};
use vuma_scg::{NodeType, SCG};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse source and return the AST. Panics if parsing fails entirely
/// (no value produced).
fn parse(source: &str) -> Program {
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.value.is_some(),
        "Parse produced no value. Errors: {:?}",
        result.errors
    );
    result.unwrap()
}

/// Parse source → AST → SCG. Returns (Program, SCG).
fn parse_and_convert(source: &str) -> (Program, SCG) {
    let program = parse(source);
    let mut converter = AstToScg::new();
    let scg = converter
        .convert(&program)
        .expect("AST → SCG conversion should succeed");
    (program, scg)
}

/// Assert that the SCG has at least one node of the given type.
fn assert_has_node_type(scg: &SCG, node_type: NodeType, label: &str) {
    let found = scg.nodes().any(|n| n.node_type == node_type);
    assert!(found, "SCG should have at least one {} node", label);
}

/// Assert that the SCG is non-empty and passes basic structural validation.
fn assert_scg_valid(scg: &SCG) {
    assert!(scg.node_count() > 0, "SCG should not be empty");
}

// ===========================================================================
// Test 1: Minimal program
// ===========================================================================

#[test]
fn test_minimal_program() {
    let source = "fn main() -> i32 { return 0; }";

    let program = parse(source);

    // Should have exactly one item (the function definition).
    assert_eq!(program.items.len(), 1, "should have one top-level item");

    // Verify it is a function named "main".
    match &program.items[0] {
        vuma_parser::Item::FnDef(fndef) => {
            assert_eq!(fndef.name, "main", "function should be named 'main'");
            assert!(
                fndef.return_type.is_some(),
                "main should have a return type annotation"
            );
            // Body should contain at least one statement (the return).
            assert!(
                !fndef.body.statements.is_empty(),
                "main body should have statements"
            );
        }
        other => panic!("expected FnDef item, got {:?}", other),
    }

    // Round-trip through SCG.
    let (_, scg) = parse_and_convert(source);
    assert_scg_valid(&scg);
}

// ===========================================================================
// Test 2: Function with parameters
// ===========================================================================

#[test]
fn test_function_with_params() {
    let source = "fn add(a: u32, b: u32) -> u32 { return a + b; }";

    let program = parse(source);

    assert_eq!(program.items.len(), 1);
    match &program.items[0] {
        vuma_parser::Item::FnDef(fndef) => {
            assert_eq!(fndef.name, "add");
            assert_eq!(fndef.params.len(), 2, "add should have 2 parameters");
            assert_eq!(fndef.params[0].name, "a");
            assert_eq!(fndef.params[1].name, "b");
            // Both params should have type annotations.
            assert!(fndef.params[0].ty.is_some(), "param 'a' should have type");
            assert!(fndef.params[1].ty.is_some(), "param 'b' should have type");
        }
        other => panic!("expected FnDef, got {:?}", other),
    }

    let (_, scg) = parse_and_convert(source);
    assert_scg_valid(&scg);
}

// ===========================================================================
// Test 3: Memory operations (allocate, deref, free)
// ===========================================================================

#[test]
fn test_memory_operations() {
    let source = r#"
        fn use_mem() {
            buf = allocate(64);
            val = *buf;
            free(buf);
        }
    "#;

    let program = parse(source);

    // Should have one function with statements for allocate, deref, and free.
    assert_eq!(program.items.len(), 1);
    match &program.items[0] {
        vuma_parser::Item::FnDef(fndef) => {
            assert_eq!(fndef.name, "use_mem");
            // Body should have at least 3 statements (allocate, deref-assign, free).
            assert!(
                fndef.body.statements.len() >= 3,
                "use_mem should have allocate, deref, and free statements (got {})",
                fndef.body.statements.len()
            );
        }
        other => panic!("expected FnDef, got {:?}", other),
    }

    let (_, scg) = parse_and_convert(source);
    assert_scg_valid(&scg);
    // SCG should reflect allocation and deallocation.
    assert_has_node_type(&scg, NodeType::Allocation, "Allocation");
    assert_has_node_type(&scg, NodeType::Deallocation, "Deallocation");
}

// ===========================================================================
// Test 4: For loop
// ===========================================================================

#[test]
fn test_for_loop() {
    let source = r#"
        fn loop_test() {
            for i in 0..10 {
                x = i;
            }
        }
    "#;

    let program = parse(source);

    assert_eq!(program.items.len(), 1);
    match &program.items[0] {
        vuma_parser::Item::FnDef(fndef) => {
            assert_eq!(fndef.name, "loop_test");
            // Body should contain a for statement.
            let has_for = fndef.body.statements.iter().any(|s| {
                matches!(s, vuma_parser::Stmt::For(_))
            });
            assert!(has_for, "body should contain a for loop");
        }
        other => panic!("expected FnDef, got {:?}", other),
    }

    let (_, scg) = parse_and_convert(source);
    assert_scg_valid(&scg);
}

// ===========================================================================
// Test 5: Nested function calls
// ===========================================================================

#[test]
fn test_nested_function_calls() {
    let source = r#"
        fn inner(x: u32) -> u32 {
            return x;
        }
        fn outer() {
            result = inner(inner(42));
        }
    "#;

    let program = parse(source);

    // Should have two function definitions.
    assert_eq!(program.items.len(), 2, "should have inner and outer functions");

    // Verify both are function definitions.
    let fn_names: Vec<&str> = program.items.iter().map(|item| {
        match item {
            vuma_parser::Item::FnDef(f) => f.name.as_str(),
            _ => "<not a fn>",
        }
    }).collect();
    assert!(fn_names.contains(&"inner"), "should have 'inner' function");
    assert!(fn_names.contains(&"outer"), "should have 'outer' function");

    let (_, scg) = parse_and_convert(source);
    assert_scg_valid(&scg);
}

// ===========================================================================
// Test 6: u32 masking — (x + y) & 4294967295
// ===========================================================================

#[test]
fn test_u32_masking() {
    let source = r#"
        fn masked_add(x: u32, y: u32) -> u32 {
            return (x + y) & 4294967295;
        }
    "#;

    let program = parse(source);

    assert_eq!(program.items.len(), 1);
    match &program.items[0] {
        vuma_parser::Item::FnDef(fndef) => {
            assert_eq!(fndef.name, "masked_add");
            // Body should have a return statement.
            let has_return = fndef.body.statements.iter().any(|s| {
                matches!(s, vuma_parser::Stmt::Return(_))
            });
            assert!(has_return, "body should contain a return statement");
        }
        other => panic!("expected FnDef, got {:?}", other),
    }

    let (_, scg) = parse_and_convert(source);
    assert_scg_valid(&scg);
}

// ===========================================================================
// Test 7: Bitwise operations — AND, OR, XOR, shifts
// ===========================================================================

#[test]
fn test_bitwise_ops() {
    let source = r#"
        fn bitwise(a: u32, b: u32) {
            c = a & b;
            d = a | b;
            e = a ^ b;
            f = a << 4;
            g = a >> 4;
        }
    "#;

    let program = parse(source);

    assert_eq!(program.items.len(), 1);
    match &program.items[0] {
        vuma_parser::Item::FnDef(fndef) => {
            assert_eq!(fndef.name, "bitwise");
            // Should have at least 5 assignment statements.
            assert!(
                fndef.body.statements.len() >= 5,
                "bitwise should have at least 5 statements, got {}",
                fndef.body.statements.len()
            );
        }
        other => panic!("expected FnDef, got {:?}", other),
    }

    let (_, scg) = parse_and_convert(source);
    assert_scg_valid(&scg);
}

// ===========================================================================
// Test 8: Pointer arithmetic — *(buf + offset)
// ===========================================================================

#[test]
fn test_pointer_arithmetic() {
    let source = r#"
        fn ptr_arith() {
            buf = allocate(64);
            offset = 4;
            val = *(buf + offset);
            free(buf);
        }
    "#;

    let program = parse(source);

    assert_eq!(program.items.len(), 1);
    match &program.items[0] {
        vuma_parser::Item::FnDef(fndef) => {
            assert_eq!(fndef.name, "ptr_arith");
            // Should have statements for allocate, offset assign, deref, and free.
            assert!(
                fndef.body.statements.len() >= 3,
                "ptr_arith should have multiple statements, got {}",
                fndef.body.statements.len()
            );
        }
        other => panic!("expected FnDef, got {:?}", other),
    }

    let (_, scg) = parse_and_convert(source);
    assert_scg_valid(&scg);
    assert_has_node_type(&scg, NodeType::Allocation, "Allocation");
    assert_has_node_type(&scg, NodeType::Deallocation, "Deallocation");
}

// ===========================================================================
// Test 9: SHA256d full program parse
// ===========================================================================

#[test]
fn test_sha256d_parse() {
    // This is the complete sha256d.vuma program. We verify that it parses
    // without errors and produces a valid AST with the expected number of
    // function definitions, and that the AST → SCG conversion succeeds.
    let source = include_str!("../../../examples/sha256d.vuma");

    let program = parse(source);

    // sha256d.vuma should define multiple functions: rotr32, ch, maj,
    // big_sigma0/1, small_sigma0/1, read_u32_be, write_u32_be, w_store,
    // w_load, sha256_init_state, sha256_init_k, sha256_transform,
    // sha256_pad_block, copy32, sha256d, main.
    let fn_count = program
        .items
        .iter()
        .filter(|item| matches!(item, vuma_parser::Item::FnDef(_)))
        .count();
    assert!(
        fn_count >= 10,
        "sha256d should define at least 10 functions, found {}",
        fn_count
    );

    // Verify "main" exists.
    let has_main = program.items.iter().any(|item| {
        matches!(item, vuma_parser::Item::FnDef(f) if f.name == "main")
    });
    assert!(has_main, "sha256d should define a 'main' function");

    // Verify "sha256d" exists.
    let has_sha256d = program.items.iter().any(|item| {
        matches!(item, vuma_parser::Item::FnDef(f) if f.name == "sha256d")
    });
    assert!(has_sha256d, "sha256d should define a 'sha256d' function");

    // Round-trip through SCG.
    let (_, scg) = parse_and_convert(source);
    assert_scg_valid(&scg);
    // The SHA256d program uses allocate/free heavily.
    assert_has_node_type(&scg, NodeType::Allocation, "Allocation");
    assert_has_node_type(&scg, NodeType::Deallocation, "Deallocation");
}

// ===========================================================================
// Test 10: Error recovery — malformed source should produce diagnostics,
//          not panic
// ===========================================================================

#[test]
fn test_error_recovery() {
    // Various malformed inputs: each should parse without panicking,
    // produce diagnostics (errors), and ideally still return a partial AST.

    let malformed_sources = &[
        // Missing semicolon after return
        "fn bad() -> i32 { return 0 }",
        // Missing closing brace
        "fn unclosed() { let x = 1;",
        // Invalid token in expression
        "fn weird() { x = @@@; }",
        // Missing function name
        "fn () { }",
        // Type in wrong position
        "fn bad() { let x = ; }",
        // Just completely garbled input
        "{{{{ fn }}}}",
    ];

    for (i, source) in malformed_sources.iter().enumerate() {
        let mut parser = Parser::new(source);
        let result = parser.parse_program();

        // The parser must not panic — we got here, so it didn't.
        // It should produce at least one diagnostic error.
        assert!(
            result.has_errors(),
            "malformed source #{} should produce parse errors:\n{}",
            i,
            source
        );

        // The error list should not be empty.
        assert!(
            !result.errors.is_empty(),
            "malformed source #{} should have non-empty error list:\n{}",
            i,
            source
        );

        // Even with errors, the parser may produce a partial AST (value
        // is Some). Whether it does depends on recovery, but it should
        // never panic.
        // If we do get a value, verify it is at least a valid Program.
        if let Some(program) = &result.value {
            // Program always has items (possibly empty) and a span.
            let _ = &program.items;
            let _ = &program.span;
        }
    }
}
