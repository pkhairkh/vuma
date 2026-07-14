//! SMT/verification tests (Wave 7).
//!
//! These tests prove that the e-graph rewrite rules are verified sound by
//! the bitvector verification framework — not by a hardcoded `verified: true`
//! flag. Each rule is exhaustively checked over all 8-bit inputs.

use vuma_codegen::bv_verify::{verify_all_rules, assert_all_rules_sound, count_verified};

#[test]
fn wave7_all_rules_verified_sound() {
    // THE Wave 7 test: every rewrite rule in the e-graph must pass
    // exhaustive bitvector verification. If any rule is unsound, this
    // test fails with a counterexample.
    let results = verify_all_rules();
    let unsound: Vec<_> = results.iter().filter(|r| !r.sound).collect();
    if !unsound.is_empty() {
        let msgs: Vec<_> = unsound.iter().map(|r| r.to_string()).collect();
        panic!("unsound rules detected:\n{}", msgs.join("\n"));
    }
}

#[test]
fn wave7_verify_count_meets_minimum() {
    let (sound, total) = count_verified();
    assert!(total >= 16, "expected at least 16 verified rules, got {}", total);
    assert_eq!(sound, total, "all {} rules should be sound, only {} are", total, sound);
}

#[test]
fn wave7_assert_all_rules_sound_api() {
    // The assert_all_rules_sound() function is the API that production code
    // can call to gate rule application on verification. It must return Ok.
    assert!(assert_all_rules_sound().is_ok(), "all rules should be sound");
}

#[test]
fn wave7_verification_evaluates_all_cases() {
    // Each 1-variable rule must evaluate 256 cases (8-bit exhaustive).
    // Each 2-variable rule must evaluate 65536 cases (256^2).
    let results = verify_all_rules();
    for r in &results {
        // 1-var rules: 256 cases. 2-var rules: 65536 cases.
        assert!(
            r.cases_evaluated == 256 || r.cases_evaluated == 65536,
            "rule {} evaluated {} cases (expected 256 or 65536)",
            r.rule_name,
            r.cases_evaluated
        );
    }
}

#[test]
fn wave7_verification_detects_unsound_rules() {
    // Sanity check: the verifier must detect deliberately unsound rules.
    use vuma_codegen::bv_verify::{verify_rule_1var, eval_binop};
    use vuma_codegen::ir::BinOpKind;

    // Claim: x + 1 == x (FALSE for all x except... let's see)
    let result = verify_rule_1var(
        "bogus",
        |x| eval_binop(BinOpKind::Add, x, 1),
        |x| x,
    );
    assert!(!result.sound, "x+1==x must be detected as unsound");
    assert!(result.counterexample.is_some(), "must provide a counterexample");
}
