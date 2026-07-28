//! # Linear-Channel HARD-FAIL Promotion Regression Test
//!
//! A previous fix eliminated the linear-channel call-site false
//! positive (the pipeline was using the SCG node index as the channel
//! `vreg` identifier instead of the handle's variable name, producing
//! spurious "use of uninitialized channel" / "channel_close on
//! uninitialized" warnings on any program with >1 channel operation).
//!
//! With the false positive eliminated, the linear-channel gate was
//! promoted from advisory-by-default (warn-only, with `--strict-ive`
//! opting into HARD-FAIL) to UNCONDITIONAL HARD-FAIL: any genuine
//! linear-channel violation returns
//! `VumaError::Transform { pass_name: "linear-channel", ... }` and
//! aborts compilation, regardless of the `--strict-ive` flag.
//!
//! ## Known limitation: parser does not emit `NodePayload::ChannelOpen`
//!
//! The pipeline's Stage 7c call site matches
//! `NodePayload::ChannelOpen(ChannelOpenNode)` /
//! `ChannelSend(ChannelSendNode)` / `ChannelRecv(ChannelRecvNode)` /
//! `ChannelClose(ChannelCloseNode)` to build the `ChannelEvent` list.
//! However, the parser (`parser/src/to_scg.rs:2386-2398`) currently
//! lowers `channel_open` / `channel_send` / `channel_recv` /
//! `channel_close` as GENERIC `ControlNode` payloads with labels
//! `call_channel_open` / `call_channel_send` / etc. — it does NOT
//! emit the dedicated `NodePayload::Channel*` variants (see the
//! comment in `to_scg.rs`: the parser does not add a dedicated
//! ChannelNode variant to the parser SCG).
//!
//! This means the linear-channel gate is currently DORMANT for
//! programs compiled through the parser — the `events` Vec is always
//! empty, `verify_linear_channels` returns an empty result, and
//! `all_linear_valid(&[])` returns `true`. The HARD-FAIL promotion
//! is therefore a NO-OP for real-world programs UNTIL a future fix
//! updates the parser to emit `NodePayload::Channel*` variants (or
//! adds an SCG transform that promotes the `ControlNode`-with-label
//! representation to the dedicated variants).
//!
//! The NEGATIVE tests below (use-after-close, double-close) are
//! marked `#[ignore]` because they cannot pass until the parser gap
//! is fixed. They document the EXPECTED behavior once the parser is
//! fixed — at which point the `#[ignore]` attribute can be removed.
//! The POSITIVE test (valid program compiles) is NOT ignored because
//! it passes today and pins that the promotion didn't break the
//! happy path.
//!
//! The corresponding UNIT-level regression tests
//! (`test_promotion_use_after_close_yields_hard_fail_violation` and
//! `test_promotion_double_close_yields_hard_fail_violation`) live in
//! `src/ive/src/borrow_region.rs` and DO pass today — they pin the
//! verifier's contract directly without going through the parser.
//!
//! See `docs/caveats.md` for the full caveat resolution history and
//! the parser-gap note.

use vuma::pipeline::{compile_with_path, CompileConfig, OptLevel, VerificationLevel, VumaError};

/// Compile `source` with the DEFAULT config (matching the production
/// pipeline: O3, Normal verification, `strict_ive: false`). This is
/// the "by default" config — the regression test must verify the gate
/// fires WITHOUT the `--strict-ive` opt-in.
fn compile_default(source: &str) -> Result<vuma::pipeline::CompilationOutput, Vec<VumaError>> {
    let cfg = CompileConfig {
        opt_level: OptLevel::O3,
        verification_level: VerificationLevel::Normal,
        // strict_ive: false is the default — explicitly set here so the
        // test's intent is crystal-clear: the linear-channel gate MUST
        // fire even when --strict-ive is OFF.
        strict_ive: false,
        stop_on_first_error: false,
        ..Default::default()
    };
    compile_with_path(source, None, &cfg)
}

/// Returns `true` iff `errors` contains a `VumaError::Transform` with
/// `pass_name == "linear-channel"` and at least one error message
/// containing `needle`.
#[allow(dead_code)]
fn has_linear_channel_transform_error(errors: &[VumaError], needle: &str) -> bool {
    errors.iter().any(|e| match e {
        VumaError::Transform {
            pass_name,
            errors: errs,
        } => pass_name == "linear-channel" && errs.iter().any(|m| m.contains(needle)),
        _ => false,
    })
}

/// Sanity check: a linearly-VALID program (open → use →
/// close) MUST compile successfully under the default config. This
/// pins that the promotion didn't accidentally over-fire on
/// legitimate channel-using programs (which was the false positive
/// the earlier fix eliminated). This test PASSES today because the
/// parser gap means the gate is dormant — but it would still need to
/// pass once the parser is fixed (the valid program must not trigger
/// a violation).
#[test]
fn linear_channel_valid_program_still_compiles() {
    let src = r#"
        transform main() -> i32 {
            ch = channel_open<i32>();
            channel_send(ch, 42);
            channel_close(ch);
            return 0;
        }
    "#;
    let result = compile_default(src);
    assert!(
        result.is_ok(),
        "open → send → close is linearly valid and must compile under \
         the default config; got errors: {:?}",
        result.err()
    );
}

/// A program with a genuine use-after-close violation
/// (open → close → recv) MUST fail to compile under the default
/// config (`strict_ive: false`). This is the canonical regression
/// test for the unconditional HARD-FAIL promotion: if the gate were
/// still advisory-by-default, this program would compile successfully
/// (with only a `vuma_log!(warn)`); with the promotion, it returns
/// `Err` with a `VumaError::Transform { pass_name: "linear-channel" }`.
///
/// The parser now emits `NodePayload::Channel*` variants for the
/// four channel builtins (see `parser/src/to_scg.rs::
/// try_emit_channel_node`), so Stage 7c's pattern-match populates the
/// `events` Vec and the gate fires on this program.
#[test]
fn linear_channel_use_after_close_fails_by_default() {
    let src = r#"
        transform main() -> i32 {
            ch = channel_open<i32>();
            channel_close(ch);
            x = channel_recv(ch);
            return x;
        }
    "#;
    let result = compile_default(src);
    let errs = result.expect_err(
        "use-after-close MUST fail to compile under the default config \
         (strict_ive: false) after the HARD-FAIL promotion",
    );
    assert!(
        has_linear_channel_transform_error(&errs, "use-after-close"),
        "expected a `VumaError::Transform {{ pass_name: \"linear-channel\", ... }}` \
         error containing 'use-after-close'; got: {:?}",
        errs
    );
}

/// A program with a double-close violation (open → close
/// → close) MUST also fail to compile under the default config. This
/// is a companion to the use-after-close test — covers the second
/// class of linear-discipline violation that the gate must catch.
///
/// The parser now emits `NodePayload::Channel*` variants, so this
/// test runs by default.
#[test]
fn linear_channel_double_close_fails_by_default() {
    let src = r#"
        transform main() -> i32 {
            ch = channel_open<i32>();
            channel_close(ch);
            channel_close(ch);
            return 0;
        }
    "#;
    let result = compile_default(src);
    let errs = result.expect_err(
        "double-close MUST fail to compile under the default config \
         (strict_ive: false) after the HARD-FAIL promotion",
    );
    assert!(
        has_linear_channel_transform_error(&errs, "double-close"),
        "expected a `VumaError::Transform {{ pass_name: \"linear-channel\", ... }}` \
         error containing 'double-close'; got: {:?}",
        errs
    );
}
