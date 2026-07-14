//! # Wave 48 — Bootstrap SCG / BD / IVE implementation tests
//!
//! Implements the test coverage required by TASKS.md Wave 48:
//!
//! 1. **Source-level smoke test** (`test_wave48_bootstrap_no_stubs`) —
//!    reads `womb/lang/ir_builder.vuma` and asserts that the three formerly-
//!    stubbed functions (`scg_construct`, `bd_infer`, `ive_verify`) no
//!    longer contain `STUB` comments or `Wave 48 STUB` markers.
//!
//! 2. **Real-logic test** (`test_wave48_bootstrap_has_real_logic`) —
//!    asserts each function contains real control flow (loops, conditionals,
//!    function calls to helpers) rather than `return 0;` / `return 1;`.
//!
//! 3. **Helper-presence test** (`test_wave48_bootstrap_has_helper_calls`) —
//!    asserts each function calls at least one helper defined in the same
//!    file (e.g., `scg_construct` calls `scg_new`/`scg_add_node`/etc.,
//!    `bd_infer` calls `bd_classify`, `ive_verify` calls
//!    `ive_bm_set`/`ive_bm_get`/`ive_op_is_def`).
//!
//! ## Why not an end-to-end test?
//!
//! An end-to-end test ("the bootstrap compiler reads `womb/lang/hello.vuma`
//! and produces exit code 0") is NOT feasible at this point because the
//! `.vuma` bootstrap compiler is not yet invokable from the Rust runtime
//! (see `wave50.rs:617-628 test_wave50_bootstrap_milestone` and
//! `wave47_bootstrap.rs` for the same limitation). These tests are
//! source-level structural checks: they verify the stubs were replaced
//! with real implementations that contain meaningful logic, but they do
//! not execute the bootstrap compiler itself. A future wave that adds a
//! runtime path to compile + link the `.vuma` files into a `vumac`
//! binary will be able to add a real end-to-end test.

use std::path::Path;

// ===========================================================================
// Helper: resolve the workspace root
// ===========================================================================

/// Resolve the workspace root from `CARGO_MANIFEST_DIR`.
///
/// The `vuma-tests` crate lives at `<workspace>/src/tests`, so the workspace
/// root is two `parent()` calls up. Falls back to `.` for `cargo test
/// --no-run` from the workspace root.
fn workspace_root() -> std::path::PathBuf {
    option_env!("CARGO_MANIFEST_DIR")
        .map(|s| {
            Path::new(s)
                .parent() // src
                .and_then(|p| p.parent()) // workspace
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| Path::new(s).to_path_buf())
        })
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}

/// Read the ir_builder.vuma source file.
fn read_ir_builder_source() -> String {
    let source_path = workspace_root()
        .join("womb")
        .join("lang")
        .join("ir_builder.vuma");
    std::fs::read_to_string(&source_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", source_path.display(), e))
}

/// Extract a single function body from the source by name.
///
/// Returns the substring starting at `fn <name>(` and ending just before
/// the next top-level `fn ` declaration (or end of file). Panics if the
/// function is not found.
fn extract_function_body(source: &str, name: &str) -> String {
    let needle = format!("fn {}(", name);
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("function `{}` not found in ir_builder.vuma", name));
    // Find the next top-level `fn ` after `start` (i.e., a `fn ` at the
    // start of a line, not indented).
    let rest = &source[start..];
    let mut end = source.len();
    // Walk past the function header line, then look for `\nfn ` at column 0.
    let after_header = rest
        .find('{')
        .unwrap_or(rest.len());
    let search_from = start + after_header + 1;
    if let Some(rel) = source[search_from..].find("\nfn ") {
        end = search_from + rel;
    }
    source[start..end].to_string()
}

// ===========================================================================
// Test 1: no STUB comments remain in the three functions
// ===========================================================================

/// Verify that `scg_construct`, `bd_infer`, and `ive_verify` no longer
/// contain `STUB` markers. The original Wave 48 audit found each function
/// was a `return 0;` / `return 1;` stub with a `// STUB` or
/// `// Wave 48 STUB` comment. After remediation, none of these markers
/// should remain.
#[test]
fn test_wave48_bootstrap_no_stubs() {
    let source = read_ir_builder_source();

    // The three functions' bodies.
    let scg = extract_function_body(&source, "scg_construct");
    let bd = extract_function_body(&source, "bd_infer");
    let ive = extract_function_body(&source, "ive_verify");

    // ── The word "STUB" must not appear in any of the three functions. ──
    assert!(
        !scg.contains("STUB"),
        "scg_construct must not contain 'STUB' — found in:\n{}",
        scg
    );
    assert!(
        !bd.contains("STUB"),
        "bd_infer must not contain 'STUB' — found in:\n{}",
        bd
    );
    assert!(
        !ive.contains("STUB"),
        "ive_verify must not contain 'STUB' — found in:\n{}",
        ive
    );

    // ── The bodies must not be the trivial stub bodies. ──
    // The original stubs were literally `return 0;` (scg_construct) or
    // `return 1;` (bd_infer, ive_verify). After remediation, each function
    // must contain MORE than just a return statement.
    assert!(
        !scg.trim().ends_with("return 0;\n}"),
        "scg_construct must not be a trivial `return 0;` stub — body:\n{}",
        scg
    );
    assert!(
        !bd.trim().ends_with("return 1;\n}"),
        "bd_infer must not be a trivial `return 1;` stub — body:\n{}",
        bd
    );
    assert!(
        !ive.trim().ends_with("return 1;\n}"),
        "ive_verify must not be a trivial `return 1;` stub — body:\n{}",
        ive
    );
}

// ===========================================================================
// Test 2: each function contains real control flow
// ===========================================================================

/// Verify each function contains real control flow: at least one `while`
/// loop (for IR / AST traversal) and at least one `if` conditional (for
/// per-opcode dispatch). This catches "real implementation" regressions
/// where the body is replaced with a constant return.
#[test]
fn test_wave48_bootstrap_has_real_logic() {
    let source = read_ir_builder_source();

    let scg = extract_function_body(&source, "scg_construct");
    let bd = extract_function_body(&source, "bd_infer");
    let ive = extract_function_body(&source, "ive_verify");

    // ── Each function must contain at least one `while` loop. ──
    // scg_construct walks AST nodes; bd_infer walks IR; ive_verify walks IR
    // (multiple passes).
    assert!(
        scg.contains("while "),
        "scg_construct must contain a `while` loop for AST traversal — body:\n{}",
        scg
    );
    assert!(
        bd.contains("while "),
        "bd_infer must contain a `while` loop for IR traversal — body:\n{}",
        bd
    );
    assert!(
        ive.contains("while "),
        "ive_verify must contain a `while` loop for IR traversal — body:\n{}",
        ive
    );

    // ── Each function must contain at least one `if` conditional. ──
    // scg_construct dispatches on AST node kind (NK_FN_DEF check);
    // bd_infer dispatches on IR opcode; ive_verify dispatches on opcode.
    assert!(
        scg.contains("if "),
        "scg_construct must contain an `if` for AST-kind dispatch — body:\n{}",
        scg
    );
    assert!(
        bd.contains("if "),
        "bd_infer must contain an `if` for opcode dispatch — body:\n{}",
        bd
    );
    assert!(
        ive.contains("if "),
        "ive_verify must contain an `if` for opcode dispatch — body:\n{}",
        ive
    );

    // ── ive_verify must have multiple passes (at least 3 `while` loops). ──
    // Pass 1 (forward) marks defined vregs; Pass 2 (backward) marks used
    // vregs; Pass 3 (forward) counts dead defs. The liveness-check loop is
    // a 4th `while`. So ive_verify should contain at least 3 `while` loops.
    let ive_while_count = ive.matches("while ").count();
    assert!(
        ive_while_count >= 3,
        "ive_verify must contain at least 3 `while` loops (forward-def, \
         backward-use, cleanup + liveness-check); found {} — body:\n{}",
        ive_while_count,
        ive
    );
}

// ===========================================================================
// Test 3: each function calls helper(s) defined in the same file
// ===========================================================================

/// Verify each function calls at least one helper defined in
/// `ir_builder.vuma`. This confirms the implementation is structured
/// (constants + helpers + main function) rather than a single monolithic
/// function with inline magic numbers.
#[test]
fn test_wave48_bootstrap_has_helper_calls() {
    let source = read_ir_builder_source();

    let scg = extract_function_body(&source, "scg_construct");
    let bd = extract_function_body(&source, "bd_infer");
    let ive = extract_function_body(&source, "ive_verify");

    // ── scg_construct must call scg_new, scg_add_node, scg_walk_block. ──
    assert!(
        scg.contains("scg_new("),
        "scg_construct must call scg_new() to allocate the SCG buffer"
    );
    assert!(
        scg.contains("scg_add_node("),
        "scg_construct must call scg_add_node() to append FunctionEntry nodes"
    );
    assert!(
        scg.contains("scg_walk_block("),
        "scg_construct must call scg_walk_block() to walk function bodies"
    );

    // ── bd_infer must call bd_classify. ──
    assert!(
        bd.contains("bd_classify("),
        "bd_infer must call bd_classify() to classify dst RepDs by opcode"
    );

    // ── ive_verify must call ive_bm_set, ive_bm_get, ive_op_is_def. ──
    assert!(
        ive.contains("ive_bm_set("),
        "ive_verify must call ive_bm_set() to mark defined/used vregs"
    );
    assert!(
        ive.contains("ive_bm_get("),
        "ive_verify must call ive_bm_get() to query the liveness bitmaps"
    );
    assert!(
        ive.contains("ive_op_is_def("),
        "ive_verify must call ive_op_is_def() to identify def-instructions"
    );

    // ── The helpers themselves must be defined in the file. ──
    let helpers = [
        "fn scg_new(",
        "fn scg_add_node(",
        "fn scg_connect(",
        "fn scg_node_kind_for(",
        "fn scg_walk_block(",
        "fn bd_classify(",
        "fn ive_bm_set(",
        "fn ive_bm_get(",
        "fn ive_bm_clear(",
        "fn ive_op_is_def(",
    ];
    for h in &helpers {
        assert!(
            source.contains(h),
            "ir_builder.vuma must define helper `{}` — not found",
            h
        );
    }
}

// ===========================================================================
// Test 4: each function uses constants defined in the file
// ===========================================================================

/// Verify the constants used by each function are defined. This catches
/// the case where the function references a constant that doesn't exist
/// (e.g., due to a typo or refactor).
#[test]
fn test_wave48_bootstrap_constants_defined() {
    let source = read_ir_builder_source();

    // SCG constants.
    let scg_consts = [
        "const SCG_NK_FUNCTION_ENTRY:",
        "const SCG_NK_COMPUTE:",
        "const SCG_NK_ACCESS:",
        "const SCG_NK_CONTROL:",
        "const SCG_NK_STATEMENT:",
        "const SCG_EK_CONTROL_FLOW:",
        "const SCG_NODE_CAP:",
        "const SCG_EDGE_CAP:",
        "const SCG_NODE_SIZE:",
        "const SCG_EDGE_SIZE:",
        "const SCG_EDGES_OFFSET:",
    ];
    for c in &scg_consts {
        assert!(
            source.contains(c),
            "ir_builder.vuma must define SCG constant `{}` — not found",
            c
        );
    }

    // BD constants.
    let bd_consts = [
        "const REPD_UNKNOWN:",
        "const REPD_I64:",
        "const REPD_BOOL:",
        "const REPD_PTR:",
        "const REPD_I32:",
        "const BD_VREG_CAP:",
    ];
    for c in &bd_consts {
        assert!(
            source.contains(c),
            "ir_builder.vuma must define BD constant `{}` — not found",
            c
        );
    }

    // IVE constants.
    let ive_consts = [
        "const IVE_VREG_CAP:",
        "const IVE_BITMAP_BYTES:",
        "const IVE_BITMAP_U64S:",
    ];
    for c in &ive_consts {
        assert!(
            source.contains(c),
            "ir_builder.vuma must define IVE constant `{}` — not found",
            c
        );
    }
}

// ===========================================================================
// Test 5: pipeline-stage header comment is updated
// ===========================================================================

/// Verify the pipeline-stage header comment block (above scg_construct)
/// describes the functions as REAL, not STUBs. The original header said
/// `Wave 48 — pipeline entry points + SCG/BD/IVE stubs` and described each
/// function as a stub. After remediation, the header should describe them
/// as real passes.
#[test]
fn test_wave48_bootstrap_header_updated() {
    let source = read_ir_builder_source();

    // The header line must NOT still say "stubs".
    assert!(
        !source.contains("Wave 48 — pipeline entry points + SCG/BD/IVE stubs"),
        "ir_builder.vuma's pipeline header must not still describe the \
         functions as stubs"
    );

    // The header line must now describe them as real passes.
    assert!(
        source.contains("Wave 48 — pipeline entry points"),
        "ir_builder.vuma's pipeline header must still exist (with updated \
         wording)"
    );

    // Each stage's description must say "REAL" (not "STUB").
    assert!(
        source.contains("scg_construct(ast)        — REAL:"),
        "pipeline header must describe scg_construct as REAL"
    );
    assert!(
        source.contains("bd_infer(ir_buf)          — REAL:"),
        "pipeline header must describe bd_infer as REAL"
    );
    assert!(
        source.contains("ive_verify(ir_buf)        — REAL:"),
        "pipeline header must describe ive_verify as REAL"
    );
}
