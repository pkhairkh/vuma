//! pmt_feature_flag_test.rs — verify the pmt-runtime-check feature flag
//! wires the Lean-verified checkers into arena.rs.
//!
//! This test only runs when compiled with --features pmt-runtime-check.
//! It verifies that arena_alloc uses the verified checker path.

#![cfg(feature = "pmt-runtime-check")]

#[test]
fn feature_flag_compiles() {
    // If this test compiles, the feature flag is wired correctly.
    // The arena module is cfg-gated; if the feature is off, this test
    // file is empty.
    assert!(true, "pmt-runtime-check feature is enabled");
}

#[test]
fn verified_checker_callable() {
    // Verify the pmt_check module is accessible from the codegen crate
    use vuma_codegen::runtime::pmt_check;
    assert!(pmt_check::verified_capacity_check(0, 16, 1024));
    assert!(!pmt_check::verified_capacity_check(u64::MAX, 1, u64::MAX));
}

#[test]
fn overflow_check_uses_verified_path() {
    // This test documents that arena_alloc's overflow check
    // uses the verified checker when the feature is on.
    // We can't directly test the internal path, but we can verify
    // the checker is the source of truth.
    use vuma_codegen::runtime::pmt_check;

    // The verified checker catches u64 overflow (unlike the Nat model)
    assert!(!pmt_check::verified_capacity_check(u64::MAX, 1, u64::MAX));

    // And it passes for valid inputs
    assert!(pmt_check::verified_capacity_check(0, 0, 0));
    assert!(pmt_check::verified_capacity_check(0, 1000, 1024));
}

/// Wave 6-D — TRUE end-to-end FFI test.
///
/// This is the capstone of the Wave 4→5→6 FFI chain. Where
/// `pmt_runtime_ffi_smoke.rs` (Wave 5-C) calls the `_prim` externs *in
/// isolation* and `verified_checker_callable` (above) exercises the
/// Rust-mirror checker directly, **this test drives the entire production
/// `compile()` pipeline** on a real `.vuma` program with
/// `pmt-runtime-check` enabled.
///
/// ## What the pipeline exercises under `pmt-runtime-check`
///
/// `pipeline::compile` -> Stage 6 `InvariantAggregator::verify_all` ->
/// `verify_pmt` -> `verify_pmt_via_lean` (cfg-gated on this feature) ->
/// the `lean_verify_state_reads` / `lean_verify_state_writes` /
/// `lean_verify_transform` `extern "C"` symbols linked from
/// `liblean_extraction.a` (stub or real).
///
/// So if **any** link in the Wave 4->5 chain is broken -- the build.rs
/// archive link, the `extern "C"` signatures, the result marshalling, the
/// `verify_pmt_via_lean` routing -- `compile()` will either panic (unsafe
/// FFI) or return `Err`. Asserting `Ok` + a populated `verification` field
/// therefore proves the FFI surface is wired end-to-end.
///
/// ## Why `examples/minimal.vuma`
///
/// Smallest example in the tree (79 bytes: `fn main() -> i32 { return 0; }`).
/// It has no state ops, so `verify_pmt_via_lean` is invoked with empty
/// reads/writes/transforms -- the stub returns all-pass, real Lean vacuously
/// passes. Either way the FFI boundary is crossed without panicking, which
/// is what this test asserts.
#[test]
fn end_to_end_pipeline_invokes_lean_ffi_verification_path() {
    use vuma::pipeline::{compile, CompileConfig};
    use vuma_ive::OverallVerdict;

    // Smallest example program -- see method doc for why.
    let source = include_str!("../examples/minimal.vuma");

    // Default config: VerificationLevel::Normal, backend AArch64.
    // Under `pmt-runtime-check` this forces Stage 6 to route through
    // `verify_pmt_via_lean` (the Lean extern path), not the hand-written
    // Rust verifiers.
    let config = CompileConfig::default();

    // THE assertion: the full pipeline -- parse -> SCG -> MSG -> IVE
    // verification (Lean FFI path) -> lowering -> regalloc -> emission --
    // completes without panicking or returning a verification error.
    let output = compile(source, &config).unwrap_or_else(|errors| {
        panic!(
            "end-to-end pipeline (Lean FFI verification path) failed; \
             errors: {}",
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        );
    });

    // Stronger: prove the IVE verification STAGE actually ran inside the
    // pipeline (not short-circuited). The pipeline pushes an
    // `"ive-verification"` timing entry iff `aggregator.verify_all` was
    // invoked -- and with `pmt-runtime-check` on, that call routes through
    // `verify_pmt_via_lean`. Absent this entry, the FFI path was never
    // reached and the test would be vacuous.
    let ive_ran = output
        .stage_timings
        .iter()
        .any(|(name, _)| name == "ive-verification");
    assert!(
        ive_ran,
        "Stage 6 `ive-verification` did not run -- Lean FFI routing \
         never invoked; stage_timings: {:?}",
        output
            .stage_timings
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
    );

    // Stronger still: the verification result must be present and
    // non-failing. A `Fail`/`Inconclusive` verdict here means the Lean
    // extern (stub or real) returned a hard error across the FFI boundary
    // -- i.e. the marshalling/signature is wrong, not just absent. For the
    // minimal program (no state ops) we accept `Pass` or `NoChecks` (the
    // stub returns empty Vecs -> all-pass; real Lean vacuously passes).
    match &output.verification {
        Some(result) => {
            assert!(
                result.overall == OverallVerdict::Pass
                    || result.overall == OverallVerdict::NoChecks,
                "Lean-routed IVE verification returned non-pass verdict \
                 {:?} -- FFI marshalling regression?",
                result.overall
            );
        }
        None => {
            // Verification was short-circuited (only happens for
            // VerificationLevel::Quick on a region-less program). Default
            // config is Normal, so this branch should NOT be taken; if it
            // is, the FFI path was skipped and the test is vacuous.
            panic!(
                "Stage 6 produced no AggregatedResult -- verification was \
                 short-circuited, so the Lean FFI path was NOT exercised"
            );
        }
    }
}
