//! # Wave 3 — Linear Channel-Type Checker Integration Tests
//!
//! These tests verify that the linear channel-type checker
//! (`vuma::pipeline::check_linear_channels_in_codegen_scg`, wired into
//! `compile_with_path` Stage 8a) correctly accepts linearly-valid
//! programs and REJECTS linearly-invalid programs as HARD compile
//! errors.
//!
//! ## What is "linear discipline" for channels?
//!
//! A channel opened by `channel_open<T>()` is a LINEAR resource: it
//! must be used (send/recv) zero or more times, then consumed exactly
//! once by `channel_close`.  The four classes of linear-discipline
//! violation are:
//!
//!   1. **Leak** — a channel is opened but never closed on some path.
//!   2. **Use-after-close** — a `channel_send` / `channel_recv` on a
//!      handle that has already been closed.
//!   3. **Double-close** — `channel_close` on a handle that is already
//!      closed (in straight-line code; a close in each branch of an
//!      `if`/`else` is NOT a double-close — only one branch runs).
//!   4. **Close-without-open** — `channel_close` on a handle that was
//!      never opened.
//!
//! All four are HARD compile errors: the pipeline refuses to emit a
//! binary for a program with any linear-discipline violation.
//!
//! ## Path-sensitivity
//!
//! The check is path-sensitive: it forks at `if`/`else`, `loop`, and
//! `switch` boundaries.  This means a `channel_close` in each branch of
//! an `if pid == 0 { ... } else { ... }` (the canonical `spawn_worker`
//! pattern) is NOT a double-close — only one branch executes at
//! runtime.  Conversely, a leak on ANY path (e.g. a channel opened
//! before an `if` with no `channel_close` in the else-branch) IS a
//! leak, even if the channel is closed on the then-path.

use vuma::pipeline::{compile_with_path, CompileConfig, OptLevel, VerificationLevel, VumaError};

/// Run `compile_with_path` on `source` with the default O3 / Normal
/// verification config (matching the production pipeline).  Returns the
/// `Result<CompilationOutput, Vec<VumaError>>`.
fn compile_source(source: &str) -> Result<vuma::pipeline::CompilationOutput, Vec<VumaError>> {
    let cfg = CompileConfig {
        opt_level: OptLevel::O3,
        verification_level: VerificationLevel::Normal,
        stop_on_first_error: false,
        ..Default::default()
    };
    compile_with_path(source, None, &cfg)
}

/// Returns `true` iff `errors` contains a `VumaError::LinearCheck` with
/// at least one violation whose message contains `needle`.
fn has_linear_error_containing(errors: &[VumaError], needle: &str) -> bool {
    errors.iter().any(|e| match e {
        VumaError::LinearCheck { errors: errs } => errs.iter().any(|m| m.contains(needle)),
        _ => false,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Positive tests — linearly-valid programs MUST compile.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn wave3_open_then_close_compiles() {
    // The simplest valid linear lifecycle: open → close.
    let src = r#"
        fn main() -> i32 {
            ch = channel_open<i32>();
            channel_close(ch);
            return 0;
        }
    "#;
    let result = compile_source(src);
    assert!(
        result.is_ok(),
        "open-then-close should compile; got errors: {:?}",
        result.err()
    );
}

#[test]
fn wave3_open_use_close_compiles() {
    // open → send → recv → close (the canonical valid lifecycle).
    // Uses spawn_worker so both parent and child close their own handle.
    let src = r#"
        fn main() -> i32 {
            ch = channel_open<i32>();
            pid = spawn_worker();
            if pid == 0 {
                x = channel_recv(ch);
                channel_close(ch);
                return x;
            }
            channel_send(ch, 42);
            channel_close(ch);
            status = wait_worker(pid);
            return status;
        }
    "#;
    let result = compile_source(src);
    assert!(
        result.is_ok(),
        "open-use-close (spawn_worker pattern) should compile; got errors: {:?}",
        result.err()
    );
}

#[test]
fn wave3_close_in_each_if_branch_compiles() {
    // A `channel_close` in each branch of an if/else is NOT a double-
    // close — only one branch executes at runtime.  This is the
    // canonical pattern that a flow-insensitive checker would false-
    // positive on; the path-sensitive check correctly accepts it.
    let src = r#"
        fn main() -> i32 {
            ch = channel_open<i32>();
            flag = 1;
            if flag == 1 {
                channel_send(ch, 1);
                channel_close(ch);
                return 1;
            } else {
                channel_send(ch, 2);
                channel_close(ch);
                return 2;
            }
        }
    "#;
    let result = compile_source(src);
    assert!(
        result.is_ok(),
        "close-in-each-branch should compile (path-sensitive); got errors: {:?}",
        result.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Negative tests — linearly-invalid programs MUST fail to compile.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn wave3_leak_open_without_close_fails() {
    // LEAK: a channel is opened but never closed.  The linear
    // discipline requires every opened channel to be closed on every
    // path; falling off the end of `main` with `ch` still open is a
    // leak.  This is the canonical violation pattern called out in the
    // Wave 3 task spec ("a channel that's opened but never closed
    // should fail to compile").
    let src = r#"
        fn main() -> i32 {
            ch = channel_open<i32>();
            return 0;
        }
    "#;
    let result = compile_source(src);
    let errs = result.expect_err("leak (open without close) should fail to compile");
    assert!(
        has_linear_error_containing(&errs, "leak"),
        "expected a linear leak error; got: {:?}",
        errs
    );
}

#[test]
fn wave3_use_after_close_fails() {
    // USE-AFTER-CLOSE: `channel_recv` on a handle that was already
    // closed.  This is a linear-discipline violation (the handle was
    // consumed by `channel_close`).
    let src = r#"
        fn main() -> i32 {
            ch = channel_open<i32>();
            channel_close(ch);
            x = channel_recv(ch);
            return x;
        }
    "#;
    let result = compile_source(src);
    let errs = result.expect_err("use-after-close should fail to compile");
    assert!(
        has_linear_error_containing(&errs, "use-after-close"),
        "expected a use-after-close error; got: {:?}",
        errs
    );
}

#[test]
fn wave3_double_close_in_straight_line_fails() {
    // DOUBLE-CLOSE: `channel_close` on a handle that is already closed
    // (in straight-line code — no branches between the two closes).
    let src = r#"
        fn main() -> i32 {
            ch = channel_open<i32>();
            channel_close(ch);
            channel_close(ch);
            return 0;
        }
    "#;
    let result = compile_source(src);
    let errs = result.expect_err("double-close should fail to compile");
    assert!(
        has_linear_error_containing(&errs, "double-close"),
        "expected a double-close error; got: {:?}",
        errs
    );
}

#[test]
fn wave3_close_without_open_fails() {
    // CLOSE-WITHOUT-OPEN: `channel_close` on a handle that was never
    // opened.  (The variable `ch` is read but never assigned by a
    // `channel_open` — the linear checker tracks this and rejects.)
    //
    // NOTE: this test may also produce a codegen "unknown variable"
    // error from the IR builder; we only assert that SOME error is
    // produced (the linear check or the codegen error both prevent
    // emission of a binary, which is the desired outcome).
    let src = r#"
        fn main() -> i32 {
            channel_close(ch);
            return 0;
        }
    "#;
    let result = compile_source(src);
    assert!(
        result.is_err(),
        "close-without-open should fail to compile (linear or codegen error)"
    );
}

#[test]
fn wave3_leak_on_one_path_fails() {
    // LEAK ON ONE PATH: the channel is closed on the then-path but not
    // on the else-path.  The path-sensitive check flags this as a leak
    // (the else-path falls off the end of `main` with `ch` still open).
    let src = r#"
        fn main() -> i32 {
            ch = channel_open<i32>();
            flag = 0;
            if flag == 1 {
                channel_close(ch);
                return 1;
            }
            return 0;
        }
    "#;
    let result = compile_source(src);
    let errs = result.expect_err("leak on one path should fail to compile");
    assert!(
        has_linear_error_containing(&errs, "leak"),
        "expected a linear leak error (else-path falls off end with ch open); got: {:?}",
        errs
    );
}

#[test]
fn wave3_send_after_close_in_branch_fails() {
    // USE-AFTER-CLOSE in straight-line code within a branch.  The
    // then-branch closes `ch` then sends on it — a use-after-close
    // that the path-sensitive check catches inside the branch.
    let src = r#"
        fn main() -> i32 {
            ch = channel_open<i32>();
            flag = 1;
            if flag == 1 {
                channel_close(ch);
                channel_send(ch, 99);
                return 1;
            }
            channel_close(ch);
            return 0;
        }
    "#;
    let result = compile_source(src);
    let errs = result.expect_err("use-after-close in branch should fail to compile");
    assert!(
        has_linear_error_containing(&errs, "use-after-close"),
        "expected a use-after-close error; got: {:?}",
        errs
    );
}
