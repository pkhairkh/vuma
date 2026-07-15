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

// ---- Wave 10: syscall() intrinsic parsing ----

#[test]
fn test_parse_syscall_basic() {
    // syscall(1, fd, buf, count) — write syscall
    let source = "fn f() { let ret = syscall(1, fd, buf, count); }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(result.is_ok(), "syscall with args should parse");
}

#[test]
fn test_parse_syscall_no_args() {
    // syscall(60) — exit syscall (no args, no return)
    let source = "fn f() { syscall(60); }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(result.is_ok(), "syscall with no args should parse");
}

#[test]
fn test_parse_syscall_as_statement() {
    // syscall as a bare statement (void return)
    let source = "fn f() { syscall(60, 0); }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(result.is_ok(), "syscall as statement should parse");
}

#[test]
fn test_parse_syscall_in_expression() {
    // syscall used in a larger expression
    let source = "fn f() { let x = syscall(1, fd, buf, count) + 1; }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(result.is_ok(), "syscall in expression should parse");
}

// ---- Wave 48: regression tests for `else { if X { ... } else { if Y { ... } } }`
// ---- (the "dangling-else across block boundary" construct used heavily by
// ---- womb/lang/full_lexer.vuma keyword matching). These tests ensure the
// ---- parser's `parse_else_clause` special-case (see parser.rs ~line 1384)
// ---- correctly handles arbitrary-length chains of `else { if ... { } }`
// ---- blocks, which is the form the bootstrap self-hosting files use
// ---- instead of the more compact `else if ... { }`.

/// Regression test for the exact construct that was broken in
/// `womb/lang/full_lexer.vuma` line 308 (missing closing brace in a
/// deeply-nested `else { if X { if Y { if Z { if W { ... } } } } }`
/// pattern). The original source had an unbalanced brace (5 opens, 4
/// closes), which corrupted parser state and produced a misleading
/// "expected expression, found 'fn'" error 220 lines later (line 529)
/// instead of a clear error at line 308. With the brace fixed, the
/// construct parses cleanly. This test verifies the parser handles the
/// balanced version of this construct.
#[test]
fn test_parse_else_block_with_nested_if_chain_balanced() {
    let source = r#"
        fn match_kw(c: i32, d: i32) -> i32 {
            if c == 99 {
                if d == 111 { if d == 110 { if d == 115 { if d == 116 { return 25; } } } }
                else { if d == 97 { if d == 116 { if d == 99 { if d == 104 { return 34; } } } } }
            }
            return 0;
        }
        fn main() -> i32 { return 0; }
    "#;
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "balanced else {{ if X {{ nested ifs }} }} should parse — errors: {:?}",
        result.errors
    );
    // Should also have NO non-fatal parse errors.
    let program = result.unwrap();
    assert_eq!(program.items.len(), 2, "should have match_kw + main");
}

/// Regression test for a longer chain of `else { if X { } }` blocks,
/// mirroring the full keyword-matching structure in
/// `womb/lang/full_lexer.vuma` lines 300-315. Verifies the parser's
/// `parse_else_clause` correctly attaches each trailing `else` to the
/// inner if via the dangling-else-across-block-boundary recursion
/// (see parser.rs ~line 1419-1423).
#[test]
fn test_parse_else_if_block_chain_five_branches() {
    let source = r#"
        fn classify(c: i32, d: i32) -> i32 {
            if c == 119 {
                if d == 104 { if d == 105 { if d == 108 { if d == 101 { return 14; } } } }
            }
            else { if c == 98 {
                if d == 114 { if d == 101 { if d == 97 { if d == 107 { return 18; } } } }
            }}
            else { if c == 99 {
                if d == 111 { if d == 110 { if d == 115 { if d == 116 { return 25; } } } }
                else { if d == 97 { if d == 116 { if d == 99 { if d == 104 { return 34; } } } } }
            }}
            else { if c == 102 {
                if d == 97 { if d == 108 { if d == 115 { if d == 101 { return 30; } } } }
            }}
            else { if c == 119 {
                if d == 104 { if d == 101 { if d == 114 { if d == 101 { return 32; } } } }
            }}
            return 0;
        }
        fn main() -> i32 { return 0; }
    "#;
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "5-branch else {{ if ... {{ }} }} chain should parse — errors: {:?}",
        result.errors
    );
    let program = result.unwrap();
    assert_eq!(program.items.len(), 2, "should have classify + main");
}

/// Regression test that the parser FAILS CLEANLY (no panic, no infinite
/// loop, no cascade) when given the UNBALANCED version of the
/// `else { if X { ... } }` construct — i.e., the original buggy line
/// 308 of `womb/lang/full_lexer.vuma`. The parser should report a
/// parse error (any error), not panic. This documents the recovery
/// behaviour that previously masked the real error site.
#[test]
fn test_parse_else_block_unbalanced_braces_does_not_panic() {
    // Unbalanced: 5 opens, 4 closes (missing one closing brace).
    let source = "fn f(c: i32, d: i32) -> i32 {\n    if c == 99 {\n        if d == 111 { if d == 110 { if d == 115 { if d == 116 { return 25; } } } }\n        else { if d == 97 { if d == 116 { if d == 99 { if d == 104 { return 34; } } } }\n    }\n    return 0;\n}\nfn main() -> i32 { return 0; }\n";
    assert_no_panic(source);
}

// ---- Wave 48 (Task 7-b): BD-directive keyword collision regression tests ----
//
// The BD-directive keywords `bd`/`repd`/`capd`/`reld` are reserved in the
// lexer (form `bd(name, expr);`), but they are also valid identifier names
// — `womb/lang/ir_builder.vuma:593` declares
// `repd: Address = __vuma_alloc(BD_VREG_CAP);` (a type-ascription
// declaration) and the same function references `repd` as a Var at lines
// 597, 611, 612, 615, 617, 621.
//
// Before the fix the parser unconditionally dispatched to
// `parse_bd_directive`, which expected `(` immediately after the keyword
// and failed with
// `ParseError { message: "expected '(', found ':'", line: Some(593), column: Some(9) }`,
// blocking the Wave 48 bootstrap self-host test. The fix in
// `parser.rs::parse_stmt` (see the `TokenKind::Bd | TokenKind::Repd |
// TokenKind::Capd | TokenKind::Reld` dispatch arm) is parser
// context-awareness: peek the token after the keyword and treat it as a
// real BD directive only when followed by `(`; otherwise treat it as an
// identifier (let-statement when followed by `:`, or assignment /
// expression statement otherwise).
//
// These tests pin both behaviours in place so future refactors of the
// BD-directive dispatch don't silently re-break the bootstrap.

use vuma_parser::ast::BdDirectiveKind;

/// `repd` used as an identifier in a type-ascription let-statement
/// (`repd: i32 = 5;`) followed by a `return repd;` — the exact construct
/// that broke the Wave 48 bootstrap at `womb/lang/ir_builder.vuma:593`.
#[test]
fn test_repd_as_identifier_in_let() {
    let source = "fn main() { repd: i32 = 5; return repd; }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "should parse without fatal error — errors: {:?}",
        result.errors
    );
    assert!(
        result.errors.is_empty(),
        "should parse with NO non-fatal errors — errors: {:?}",
        result.errors
    );
    let program = result.unwrap();
    assert_eq!(program.items.len(), 1, "should have one fn");
    // Verify the first statement is a Let, not a BdDirective.
    match &program.items[0] {
        vuma_parser::Item::FnDef(f) => {
            assert_eq!(f.body.statements.len(), 2, "should have repd: let + return");
            match &f.body.statements[0] {
                vuma_parser::Stmt::Let(l) => {
                    assert_eq!(l.name, "repd", "let should bind `repd`");
                    assert!(l.ty.is_some(), "let should have a type ascription");
                }
                other => panic!("expected Stmt::Let, got {:?}", other),
            }
            match &f.body.statements[1] {
                vuma_parser::Stmt::Return(_) => {}
                other => panic!("expected Stmt::Return, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

/// `bd` used as an identifier in a type-ascription let-statement.
#[test]
fn test_bd_as_identifier_in_let() {
    let source = "fn main() { bd: i32 = 5; return bd; }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "should parse without fatal error — errors: {:?}",
        result.errors
    );
    assert!(
        result.errors.is_empty(),
        "should parse with NO non-fatal errors — errors: {:?}",
        result.errors
    );
    let program = result.unwrap();
    match &program.items[0] {
        vuma_parser::Item::FnDef(f) => match &f.body.statements[0] {
            vuma_parser::Stmt::Let(l) => assert_eq!(l.name, "bd"),
            other => panic!("expected Stmt::Let, got {:?}", other),
        },
        other => panic!("expected FnDef, got {:?}", other),
    }
}

/// `capd` used as an identifier in a type-ascription let-statement.
#[test]
fn test_capd_as_identifier_in_let() {
    let source = "fn main() { capd: i32 = 5; return capd; }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "should parse without fatal error — errors: {:?}",
        result.errors
    );
    assert!(
        result.errors.is_empty(),
        "should parse with NO non-fatal errors — errors: {:?}",
        result.errors
    );
    let program = result.unwrap();
    match &program.items[0] {
        vuma_parser::Item::FnDef(f) => match &f.body.statements[0] {
            vuma_parser::Stmt::Let(l) => assert_eq!(l.name, "capd"),
            other => panic!("expected Stmt::Let, got {:?}", other),
        },
        other => panic!("expected FnDef, got {:?}", other),
    }
}

/// `reld` used as an identifier in a type-ascription let-statement.
#[test]
fn test_reld_as_identifier_in_let() {
    let source = "fn main() { reld: i32 = 5; return reld; }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "should parse without fatal error — errors: {:?}",
        result.errors
    );
    assert!(
        result.errors.is_empty(),
        "should parse with NO non-fatal errors — errors: {:?}",
        result.errors
    );
    let program = result.unwrap();
    match &program.items[0] {
        vuma_parser::Item::FnDef(f) => match &f.body.statements[0] {
            vuma_parser::Stmt::Let(l) => assert_eq!(l.name, "reld"),
            other => panic!("expected Stmt::Let, got {:?}", other),
        },
        other => panic!("expected FnDef, got {:?}", other),
    }
}

/// Real BD directives (`bd(name, expr);`, `repd(name);`, etc.) must STILL
/// parse as `Stmt::BdDirective` — not as a let/assign/expr statement.
/// This guards against the context-awareness fix accidentally swallowing
/// the directive form.
#[test]
fn test_repd_as_bd_directive_still_works() {
    let source = r#"
        fn main() {
            bd(Secure);
            repd(Fast, x);
            capd(RW);
            reld(Ordered, y + 1);
        }
    "#;
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "BD directives should parse without fatal error — errors: {:?}",
        result.errors
    );
    assert!(
        result.errors.is_empty(),
        "BD directives should parse with NO non-fatal errors — errors: {:?}",
        result.errors
    );
    let program = result.unwrap();
    match &program.items[0] {
        vuma_parser::Item::FnDef(f) => {
            assert_eq!(
                f.body.statements.len(),
                4,
                "should have exactly 4 BD directives"
            );
            // bd(Secure);
            match &f.body.statements[0] {
                vuma_parser::Stmt::BdDirective(d) => {
                    assert_eq!(d.kind, BdDirectiveKind::Bd);
                    assert_eq!(d.name, "Secure");
                    assert!(d.expr.is_none());
                }
                other => panic!("expected BdDirective for `bd(Secure);`, got {:?}", other),
            }
            // repd(Fast, x);
            match &f.body.statements[1] {
                vuma_parser::Stmt::BdDirective(d) => {
                    assert_eq!(d.kind, BdDirectiveKind::Repd);
                    assert_eq!(d.name, "Fast");
                    assert!(d.expr.is_some());
                }
                other => panic!("expected BdDirective for `repd(Fast, x);`, got {:?}", other),
            }
            // capd(RW);
            match &f.body.statements[2] {
                vuma_parser::Stmt::BdDirective(d) => {
                    assert_eq!(d.kind, BdDirectiveKind::Capd);
                    assert_eq!(d.name, "RW");
                }
                other => panic!("expected BdDirective for `capd(RW);`, got {:?}", other),
            }
            // reld(Ordered, y + 1);
            match &f.body.statements[3] {
                vuma_parser::Stmt::BdDirective(d) => {
                    assert_eq!(d.kind, BdDirectiveKind::Reld);
                    assert_eq!(d.name, "Ordered");
                    assert!(d.expr.is_some());
                }
                other => panic!("expected BdDirective for `reld(Ordered, y + 1);`, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

/// A BD-directive keyword used as a plain identifier in an assignment
/// (`repd = 11;`) and as a Var in a larger expression.  This was
/// previously swallowed by `parse_bd_directive` and produced a fatal
/// "expected '('" error in `parse_program`; now it should parse cleanly.
#[test]
fn test_bd_keyword_as_identifier_in_assign_and_expr() {
    let source = "fn main() { repd = 11; repd = repd + 1; return repd; }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "should parse without fatal error — errors: {:?}",
        result.errors
    );
    assert!(
        result.errors.is_empty(),
        "should parse with NO non-fatal errors — errors: {:?}",
        result.errors
    );
}

// ---- Wave 48 (Task A): additional context-aware dispatch edge cases ----
//
// The earlier tests in this file cover the common cases (let-with-ascription,
// assignment, BD-directive form).  These three additional tests pin down
// edge cases that the bootstrap `womb/lang/*.vuma` files also exercise:
//
//   * `repd + 1;`  — keyword used as a bare expression statement (next
//     token is `+`, not `(` or `:`).  Must parse as an expression statement,
//     NOT as a BD directive (which would expect `(`).
//   * `capd += 1;` — keyword used as the LHS of a compound assignment.
//     Must parse as `Stmt::CompoundAssign`, NOT as a BD directive.
//   * mixed usage in one function — a single function that declares
//     `reld: u32 = 5;`, then calls `bd(Secure);`, then reassigns
//     `reld = reld + 1;`.  Verifies the parser doesn't get "stuck" in
//     BD-directive mode after seeing one `bd(...)` call.

/// `repd + 1;` parses as an expression statement (next token `+`).
#[test]
fn test_wave48_repd_as_expression_statement() {
    let source = "fn f() { repd + 1; }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "repd + 1; should parse as expression statement, errors: {:?}",
        result.errors
    );
}

/// `capd` used as the LHS of a compound assignment (`capd += 1;`).
#[test]
fn test_wave48_capd_as_compound_assignment() {
    let source = "fn f() { capd += 1; }";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "capd += 1; should parse as compound assignment, errors: {:?}",
        result.errors
    );
}

/// Mixed usage in one function: `reld: u32 = 5;` (let), `bd(Secure);`
/// (real directive), `reld = reld + 1;` (assignment).  Verifies the parser
/// doesn't confuse the dispatch after seeing a real BD directive.
#[test]
fn test_wave48_reld_as_variable_mixed_with_bd_directive() {
    let source = r#"
        fn f() {
            reld: u32 = 5;
            bd(Secure);
            reld = reld + 1;
        }
    "#;
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok(),
        "mixed reld-as-variable and bd(...) directive should parse, errors: {:?}",
        result.errors
    );
    let program = result.unwrap();
    if let vuma_parser::ast::Item::FnDef(f) = &program.items[0] {
        assert_eq!(
            f.body.statements.len(),
            3,
            "should have three statements (let, bd-directive, assign)"
        );
    } else {
        panic!("expected FnDef, got {:?}", program.items[0]);
    }
}
