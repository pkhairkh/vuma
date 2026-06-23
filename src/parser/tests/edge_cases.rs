//! Edge-case tests for the VUMA parser (Wave 14 — parser fuzzing harness).
//!
//! These tests verify that the parser never panics on tricky or malformed
//! inputs and handles boundary conditions correctly.

use vuma_parser::Parser;

/// Helper: assert that parsing `source` does not panic (Ok or Err is fine).
///
/// Runs in a dedicated thread with a 32 MB stack so that deeply-nested
/// (but legitimate) inputs like 50-level parentheses do not trigger a
/// hard stack-overflow abort in debug builds where stack frames are large.
fn assert_no_panic(source: &str) {
    let src = source.to_string();
    let src_for_msg = src.clone();
    let handle = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut parser = Parser::new(&src);
                let _ = parser.parse_program();
            }))
        })
        .expect("failed to spawn parser thread");
    let result = handle.join().expect("parser thread panicked");
    assert!(result.is_ok(), "parser panicked on input: {:?}", src_for_msg);
}

/// Helper: assert that parsing `source` succeeds (Ok with or without errors).
fn assert_parses(source: &str) {
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "parser returned fatal error on: {:?}\nerrors: {:?}",
        source,
        result.errors
    );
}

// ---- Deeply nested parentheses/braces ----

#[test]
fn edge_deeply_nested_parens() {
    let depth = 50;
    let source = format!("{}0{}", "(".repeat(depth), ")".repeat(depth));
    assert_no_panic(&source);
}

#[test]
fn edge_deeply_nested_braces() {
    let mut inner = String::from("0");
    for _ in 0..30 {
        inner = format!("{{ let x = {};", inner);
    }
    let closing = "}".repeat(30);
    let source = format!("fn f() {}{}", inner, closing);
    assert_no_panic(&source);
}

#[test]
fn edge_deeply_nested_brackets() {
    let depth = 30;
    let source = format!("let x = {}0{}", "[".repeat(depth), "]".repeat(depth));
    assert_no_panic(&format!("fn f() {{ {} }}", source));
}

#[test]
fn edge_unmatched_closing_parens() {
    assert_no_panic(")))))))");
}

#[test]
fn edge_unmatched_closing_braces() {
    assert_no_panic("}}}}}}}}");
}

// ---- Unicode identifiers ----

#[test]
fn edge_unicode_identifier() {
    // Unicode input produces Error tokens — the parser must not panic.
    assert_no_panic("let \u{00e9} = 1;");
    assert_no_panic("let \u{4e16}\u{754c} = 2;");
    assert_no_panic("\u{03b1} + \u{03b2}"); // Greek alpha + beta
}

#[test]
fn edge_unicode_in_string() {
    assert_parses("let x = \"\u{1f600}\";");
}

// ---- Very long identifiers (1KB+) ----

#[test]
fn edge_very_long_identifier() {
    let long_name = "a".repeat(2048);
    let source = format!("let {} = 0;", long_name);
    assert_no_panic(&source);
}

#[test]
fn edge_very_long_type_name() {
    let long_name = "T".repeat(2048);
    let source = format!("let x: {} = 0;", long_name);
    assert_no_panic(&source);
}

// ---- Consecutive operators ----

#[test]
fn edge_consecutive_shift_right() {
    assert_no_panic("let x = 1 >> 2;");
    assert_no_panic("let x = 1 >> 2 >> 3;");
}

#[test]
fn edge_consecutive_shift_left() {
    assert_no_panic("let x = 1 << 2;");
    assert_no_panic("let x = 1 << 2 << 3;");
}

#[test]
fn edge_triple_equals() {
    // `===` is tokenised as `==` then `=`
    assert_no_panic("let x = 1 === 2;");
}

#[test]
fn edge_mixed_operators() {
    assert_no_panic("let x = 1 + - * & | ^ ~ ! @ << >>;");
    assert_no_panic("x += -= *= /=;");
}

#[test]
fn edge_operator_soup() {
    assert_no_panic(">>>===!==<=>=<=>>");
    assert_no_panic("..===..=...");
    assert_no_panic("&&||!&&||!");
}

// ---- Empty programs / only comments ----

#[test]
fn edge_empty_program() {
    let mut parser = Parser::new("");
    let result = parser.parse_program();
    assert!(result.is_ok());
    let program = result.unwrap();
    assert!(program.items.is_empty());
}

#[test]
fn edge_only_whitespace() {
    let mut parser = Parser::new("   \n\t  \n  ");
    let result = parser.parse_program();
    assert!(result.is_ok());
    assert!(result.unwrap().items.is_empty());
}

#[test]
fn edge_only_line_comment() {
    let mut parser = Parser::new("// this is a comment\n");
    let result = parser.parse_program();
    assert!(result.is_ok());
    assert!(result.unwrap().items.is_empty());
}

#[test]
fn edge_only_block_comment() {
    let mut parser = Parser::new("/* block comment */");
    let result = parser.parse_program();
    assert!(result.is_ok());
    assert!(result.unwrap().items.is_empty());
}

#[test]
fn edge_multiple_comments() {
    let source = "// comment 1\n/* comment 2 */\n// comment 3\n";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(result.is_ok());
    assert!(result.unwrap().items.is_empty());
}

#[test]
fn edge_only_doc_comments() {
    // Doc comments are emitted as tokens; parser should handle them gracefully
    let source = "/// doc comment\n//! module doc\n";
    assert_no_panic(source);
}

// ---- Mix of all VUMA keywords in unusual positions ----

#[test]
fn edge_keywords_as_expressions() {
    assert_no_panic("region = 1;");
    assert_no_panic("ptr = 2;");
    assert_no_panic("alloc = 3;");
    assert_no_panic("free = 4;");
    assert_no_panic("cast = 5;");
    assert_no_panic("read = 6;");
    assert_no_panic("write = 7;");
    assert_no_panic("safe = 8;");
    assert_no_panic("unsafe = 9;");
    assert_no_panic("bd = 10;");
    assert_no_panic("repd = 11;");
    assert_no_panic("capd = 12;");
    assert_no_panic("reld = 13;");
    assert_no_panic("self = 14;");
    assert_no_panic("super = 15;");
    assert_no_panic("lock = 16;");
    assert_no_panic("unlock = 17;");
    assert_no_panic("channel = 18;");
    assert_no_panic("send = 19;");
    assert_no_panic("recv = 20;");
}

#[test]
fn edge_keywords_in_unusual_positions() {
    // Keywords as type names
    assert_no_panic("fn f(x: fn) {}");
    // Keywords in match patterns
    assert_no_panic("match x { struct => 1, enum => 2 }");
}

#[test]
fn edge_all_keywords_sequential() {
    let keywords = [
        "fn", "let", "pub", "crate", "ptr", "region", "alloc", "allocate", "free", "derive",
        "cast", "read", "write", "sync", "if", "else", "while", "for", "return", "struct", "enum",
        "match", "unsafe", "safe", "bd", "repd", "capd", "reld", "import", "export", "mod", "use",
        "self", "super", "async", "await", "spawn", "lock", "unlock", "channel", "send", "recv",
        "true", "false", "null", "as", "sizeof", "alignof", "break", "continue", "where", "impl",
        "trait", "type", "const", "static", "mut", "ref",
    ];
    let source = keywords.join(";\n");
    assert_no_panic(&source);
}

// ---- Expression depth limit ----

#[test]
fn edge_expression_depth_limit() {
    let depth = 300;
    let source = format!("let x = {}1{}", "+(".repeat(depth), ")".repeat(depth));
    assert_no_panic(&format!("fn f() {{ {} }}", source));
}

// ---- Incomplete constructs ----

#[test]
fn edge_incomplete_fn() {
    assert_no_panic("fn");
    assert_no_panic("fn(");
    assert_no_panic("fn foo(");
    assert_no_panic("fn foo()");
    assert_no_panic("fn foo() {");
}

#[test]
fn edge_incomplete_struct() {
    assert_no_panic("struct");
    assert_no_panic("struct S");
    assert_no_panic("struct S {");
    assert_no_panic("struct S { x:");
    assert_no_panic("struct S { x: u32,");
}

#[test]
fn edge_incomplete_region() {
    assert_no_panic("region");
    assert_no_panic("region x");
    assert_no_panic("region x =");
    assert_no_panic("region x = allocate(");
    assert_no_panic("region x = allocate(1024");
}

#[test]
fn edge_garbage_null_bytes() {
    assert_no_panic("\0\0\0");
}

#[test]
fn edge_garbage_hashes_and_dollars() {
    assert_no_panic("###$$$@@@");
}

#[test]
fn edge_garbage_questions() {
    assert_no_panic("???!!!");
}

#[test]
fn edge_garbage_hex_like() {
    assert_no_panic("0x0x0x");
}

#[test]
fn edge_null_bytes_in_source() {
    assert_no_panic("let x = \0;");
}

// ---- New edge-case tests: empty function body, nested let, unsafe, loop ----

#[test]
fn test_parse_empty_function_body() {
    let mut parser = Parser::new("fn foo() {}");
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "empty function body should parse successfully"
    );
    let program = result.unwrap();
    assert_eq!(program.items.len(), 1, "should have exactly one item");
}

#[test]
fn test_parse_nested_let_bindings() {
    let source = "fn f() { let x = 1; let y = x; let z = y; }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "nested let bindings should parse successfully"
    );
    let program = result.unwrap();
    assert_eq!(program.items.len(), 1, "should have exactly one function");
}

#[test]
fn test_parse_unsafe_block() {
    // `unsafe` is a keyword; the parser should handle it without panicking.
    let source = "fn f() { unsafe { let x = 1; } }";
    assert_no_panic(source);
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "unsafe block inside function should parse without fatal error"
    );
}

#[test]
fn test_parse_loop_keyword() {
    let source = "fn f() { loop { break; } }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(result.is_ok(), "loop with break should parse successfully");
    let program = result.unwrap();
    assert_eq!(program.items.len(), 1, "should have exactly one function");
}
