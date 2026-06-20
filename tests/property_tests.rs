//! # Property-Based Tests for VUMA's IVE (Invariant Verification Engine)
//!
//! These tests verify that VUMA's static analysis (the IVE) and the broader
//! compilation pipeline behave correctly when presented with programs that
//! exercise specific memory-safety properties:
//!
//! 1. **Use-after-free detection** — reading freed memory
//! 2. **Buffer overflow (write)** — storing past the end of an allocation
//! 3. **Buffer overflow (read)** — loading past the end of an allocation
//! 4. **Double-free detection** — freeing the same allocation twice
//! 5. **Memory leak detection** — allocating without freeing
//! 6. **Null pointer dereference** — dereferencing the null literal
//! 7. **Uninitialized memory read** — loading from an allocation before storing
//! 8. **Valid program passes** — well-formed programs must compile (no false
//!    negatives that block compilation)
//! 9. **Multiple allocations + correct frees** — several allocations paired
//!    with their matching frees compile cleanly
//! 10. **Nested function calls with memory** — passing pointers through
//!     function call chains
//! 11. **Pointer arithmetic within bounds** — accessing `buf + N` for `N`
//!     within the allocation size
//! 12. **Conditional free** — `if` branches that free on both paths or only
//!     on one path
//! 13. **Loop allocation** — allocating/freeing inside a `while` body
//! 14. **Function returns allocated memory** — callee returns a freshly
//!     allocated buffer that the caller must free
//! 15. **Struct field access** — typed loads/stores at struct field offsets
//!
//! ## IVE Capability Notes (as of this build)
//!
//! Probing the IVE against the small in-tree programs in this file
//! established the following *current* detection capabilities, which the
//! tests below reflect:
//!
//! | Property                          | IVE catches? | Notes |
//! |-----------------------------------|--------------|-------|
//! | Double-free                       | YES          | Cleanup invariant, `Violated` status, counterexample description mentions "double free" |
//! | Memory leak (no `free` at all)    | YES          | Liveness invariant, `Violated` status, message mentions "leak" |
//! | Use-after-free                    | NO           | Pipeline still compiles; flagged here as known gap |
//! | Buffer overflow (read or write)   | NO           | Pipeline still compiles; flagged here as known gap |
//! | Null pointer dereference          | NO           | Pipeline still compiles; flagged here as known gap |
//! | Uninitialized read                | NO           | Pipeline still compiles; flagged here as known gap |
//! | Valid program (alloc + free)      | false +      | Liveness invariant reports a spurious "Resource leak" — known limitation |
//!
//! The liveness invariant's spurious "Resource leak" report on programs
//! that *do* call `free(buf)` is a documented IVE limitation: the
//! deallocation-node → allocation-node link is not always populated by
//! the SCG builder, so `LivenessVerifier` cannot see the matching free.
//! This is why several "valid program" tests below run with
//! [`VerificationLevel::None`] — they verify that the parser, SCG
//! builder, IR lowering, register allocator, and ELF emitter all
//! succeed end-to-end without the IVE's spurious leak report blocking
//! the build.
//!
//! ## SCG Builder Notes
//!
//! Probing also revealed SCG-builder behaviours that the assertions
//! below accommodate:
//!
//! - A dereferencing *read* (`val = *buf;`) does not always produce an
//!   `Access` node in the SCG; only writes (`*buf = N;`) reliably do.
//! - A `free` of a pointer that was returned from another function
//!   (e.g., `b = make_buf(); free(b);`) does not produce a
//!   `Deallocation` node, because the SCG builder cannot track the
//!   pointer-to-allocation link across function returns.
//! - Programs with loops or function calls produce a cycle in the SCG
//!   that the SCG→MSG converter rejects; the pipeline logs this as a
//!   "soft" error but still returns `Err`, so loop/call programs are
//!   tested via the parser+SCG path only.

use vuma::pipeline::{compile, CompileConfig, OptLevel, VerificationLevel, VumaError};
use vuma_ive::{InvariantKind, VerificationStatus};
use vuma_parser::{AstToScg, Parser};
use vuma_scg::{NodePayload, NodeType, SCG};

// ═══════════════════════════════════════════════════════════════════════════
// Helper utilities
// ═══════════════════════════════════════════════════════════════════════════

/// Outcome of a full pipeline run on a source program.
#[derive(Debug)]
struct CompileOutcome {
    /// `true` if the pipeline produced a binary, `false` if it returned
    /// any errors (verification or otherwise).
    success: bool,
    /// All errors (if any) collected by the pipeline, in stage order.
    errors: Vec<VumaError>,
    /// The IVE's aggregated verification result, if verification ran.
    verification: Option<vuma_ive::AggregatedResult>,
    /// Size of the emitted binary (0 on failure).
    binary_len: usize,
}

#[allow(dead_code)]
impl CompileOutcome {
    /// `true` iff at least one invariant's status was `Violated`.
    fn has_violated_invariant(&self) -> bool {
        self.verification
            .as_ref()
            .map(|v| {
                v.per_invariant
                    .iter()
                    .any(|p| matches!(p.result.status, VerificationStatus::Violated { .. }))
            })
            .unwrap_or(false)
    }

    /// Find the per-invariant result for a given invariant kind.
    fn invariant(&self, kind: InvariantKind) -> Option<&vuma_ive::invariant_aggregator::PerInvariantResult> {
        self.verification
            .as_ref()
            .and_then(|v| v.per_invariant.iter().find(|p| p.kind == kind))
    }

    /// `true` iff the given invariant was `Violated`.
    fn invariant_violated(&self, kind: InvariantKind) -> bool {
        self.invariant(kind)
            .map(|p| matches!(p.result.status, VerificationStatus::Violated { .. }))
            .unwrap_or(false)
    }

    /// Extract the counterexample description for a violated invariant,
    /// or `None` if the invariant was not violated.
    fn violation_description(&self, kind: InvariantKind) -> Option<String> {
        self.invariant(kind).and_then(|p| {
            if let VerificationStatus::Violated { counterexample } = &p.result.status {
                Some(counterexample.description.clone())
            } else {
                None
            }
        })
    }

    /// `true` iff the pipeline stage `stage` produced an error.
    fn stage_failed(&self, stage: &str) -> bool {
        self.errors.iter().any(|e| e.stage() == stage)
    }
}

/// Run the full VUMA pipeline on `source` at the given verification level.
fn run_pipeline(source: &str, level: VerificationLevel) -> CompileOutcome {
    let cfg = CompileConfig {
        opt_level: OptLevel::O0,
        verification_level: level,
        stop_on_first_error: false,
        ..Default::default()
    };
    match compile(source, &cfg) {
        Ok(out) => CompileOutcome {
            success: true,
            errors: Vec::new(),
            verification: out.verification,
            binary_len: out.binary.len(),
        },
        Err(errs) => {
            // The pipeline returns Err(Vec<VumaError>) when any stage
            // failed OR when non-fatal errors were collected (e.g.
            // SCG→MSG cycle detection is a "soft" error that is still
            // pushed onto the error list). Extract the IVE result if
            // present so callers can still inspect it.
            let verification = errs.iter().find_map(|e| {
                if let VumaError::Verification { result } = e {
                    Some(result.clone())
                } else {
                    None
                }
            });
            CompileOutcome {
                success: false,
                errors: errs,
                verification,
                binary_len: 0,
            }
        }
    }
}

/// Parse `source` and convert AST → SCG. Returns `Ok(SCG)` on success or
/// `Err(message)` describing the first parse / conversion error.
///
/// This is the lightweight "front-end only" path: it bypasses the SCG→MSG
/// conversion that fails on programs with loops or function calls (cycle
/// detection), allowing us to verify that the parser and SCG builder
/// accept a wide variety of memory-safety test programs.
fn parse_and_build_scg(source: &str) -> Result<SCG, String> {
    let mut parser = Parser::new(source);
    let parse_result = parser.parse_program();
    if parse_result.has_errors() {
        let first = &parse_result.errors[0];
        return Err(format!(
            "parse: {} error(s); first: {}",
            parse_result.errors.len(),
            first
        ));
    }
    let ast = parse_result
        .value
        .ok_or_else(|| "parse: no AST produced".to_string())?;
    let mut converter = AstToScg::new();
    converter
        .convert(&ast)
        .map_err(|e| format!("ast-to-scg: {}", e))
}

/// Count allocation nodes in an SCG.
fn count_allocations(scg: &SCG) -> usize {
    scg.nodes()
        .filter(|n| n.node_type == NodeType::Allocation)
        .count()
}

/// Count deallocation nodes in an SCG.
fn count_deallocations(scg: &SCG) -> usize {
    scg.nodes()
        .filter(|n| n.node_type == NodeType::Deallocation)
        .count()
}

/// Count access (load/store) nodes in an SCG.
fn count_accesses(scg: &SCG) -> usize {
    scg.nodes()
        .filter(|n| n.node_type == NodeType::Access)
        .count()
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 1: Use-after-free detection
// ═══════════════════════════════════════════════════════════════════════════

/// Source for a use-after-free: allocate, free, then dereference.
const UAF_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(64);
        free(buf);
        val: i32 = *buf;
        return val;
    }
"#;

#[test]
fn test_use_after_free_parses_and_builds_scg() {
    // The parser and SCG builder must accept the use-after-free program.
    // (The IVE itself does not currently flag UAF — see
    // `test_use_after_free_ive_known_gap` for the gap documentation.)
    let scg = parse_and_build_scg(UAF_SOURCE).expect("UAF program must parse + build SCG");
    assert!(scg.node_count() > 0, "expected non-empty SCG");
    assert!(count_allocations(&scg) >= 1, "expected >=1 allocation");
    assert!(count_deallocations(&scg) >= 1, "expected >=1 deallocation");
    // Note: the SCG builder does not currently emit an Access node for
    // pure reads (`val = *buf;`), so we don't assert on access count.
}

#[test]
fn test_use_after_free_compiles_without_verification() {
    // With verification disabled, the pipeline must compile the
    // use-after-free program all the way to a binary (the IVE does not
    // block it; the codegen path doesn't itself check for UAF).
    let outcome = run_pipeline(UAF_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "UAF should compile with verification off; errors: {:?}",
        outcome
            .errors
            .iter()
            .map(|e| e.stage().to_string())
            .collect::<Vec<_>>()
    );
    assert!(outcome.binary_len > 0, "expected non-empty binary");
}

#[test]
fn test_use_after_free_ive_known_gap() {
    // Documented IVE limitation: the IVE does not currently detect
    // use-after-free. This test asserts the *current* behaviour so that
    // any future improvement to the IVE will cause this test to fail
    // (prompting an update to remove the gap documentation). When the
    // IVE gains UAF detection, replace the body of this test with
    // `assert!(outcome.invariant_violated(InvariantKind::Cleanup));`.
    let outcome = run_pipeline(UAF_SOURCE, VerificationLevel::Normal);
    // The pipeline will return Err because of the spurious liveness
    // "leak" report (see module docs), so `success` is false — but the
    // cleanup invariant should NOT be marked Violated on a UAF (only
    // on actual double-frees).
    let cleanup_violated = outcome.invariant_violated(InvariantKind::Cleanup);
    assert!(
        !cleanup_violated,
        "IVE cleanup invariant should not flag UAF as a cleanup violation \
         (it isn't a double-free). IVE currently does not catch UAF — \
         see module docs."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 2: Buffer overflow (write past end)
// ═══════════════════════════════════════════════════════════════════════════

const OVERFLOW_WRITE_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(8);
        *(buf + 100) = 42;
        free(buf);
        return 0;
    }
"#;

#[test]
fn test_buffer_overflow_write_parses_and_builds_scg() {
    let scg = parse_and_build_scg(OVERFLOW_WRITE_SOURCE)
        .expect("overflow-write program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // The write `*(buf + 100) = 42` should produce an Access node.
    assert!(count_accesses(&scg) >= 1, "expected >=1 access (the write)");
}

#[test]
fn test_buffer_overflow_write_compiles_without_verification() {
    let outcome = run_pipeline(OVERFLOW_WRITE_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "overflow-write should compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

#[test]
fn test_buffer_overflow_write_ive_known_gap() {
    // IVE does not currently perform bounds checking against the
    // allocation size. Document this gap.
    let outcome = run_pipeline(OVERFLOW_WRITE_SOURCE, VerificationLevel::Normal);
    // IVE may report the spurious liveness leak, but no invariant
    // should specifically identify this as a bounds violation.
    let any_violation_mentions_bounds = outcome
        .verification
        .as_ref()
        .map(|v| {
            v.per_invariant.iter().any(|p| {
                let msg = p.result.message.to_lowercase();
                let desc = if let VerificationStatus::Violated { counterexample } = &p.result.status {
                    counterexample.description.to_lowercase()
                } else {
                    String::new()
                };
                msg.contains("bound") || msg.contains("overflow") || desc.contains("bound") || desc.contains("overflow")
            })
        })
        .unwrap_or(false);
    assert!(
        !any_violation_mentions_bounds,
        "IVE does not currently flag buffer overflows as bounds violations. \
         If this assertion fails, the IVE has gained bounds checking — \
         update the module docs."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 3: Buffer overflow (read past end)
// ═══════════════════════════════════════════════════════════════════════════

const OVERFLOW_READ_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(8);
        val: i32 = *(buf + 100);
        free(buf);
        return val;
    }
"#;

#[test]
fn test_buffer_overflow_read_parses_and_builds_scg() {
    let scg = parse_and_build_scg(OVERFLOW_READ_SOURCE)
        .expect("overflow-read program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // Note: pure reads via `*(buf + N)` do not currently produce
    // Access nodes in the SCG, so we don't assert on access count.
}

#[test]
fn test_buffer_overflow_read_compiles_without_verification() {
    let outcome = run_pipeline(OVERFLOW_READ_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "overflow-read should compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 4: Double-free detection (IVE catches this)
// ═══════════════════════════════════════════════════════════════════════════

const DOUBLE_FREE_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(64);
        free(buf);
        free(buf);
        return 0;
    }
"#;

#[test]
fn test_double_free_parses_and_builds_scg() {
    let scg = parse_and_build_scg(DOUBLE_FREE_SOURCE)
        .expect("double-free program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    assert_eq!(
        count_deallocations(&scg),
        2,
        "expected exactly 2 deallocation nodes for double-free"
    );
}

#[test]
fn test_ive_detects_double_free() {
    // The IVE *does* detect double-free, via the Cleanup invariant.
    // The pipeline should return Err with a Verification error whose
    // Cleanup invariant status is Violated.
    let outcome = run_pipeline(DOUBLE_FREE_SOURCE, VerificationLevel::Normal);
    assert!(
        outcome.stage_failed("ive-verification"),
        "double-free must cause an IVE verification failure; got errors: {:?}",
        outcome
            .errors
            .iter()
            .map(|e| e.stage().to_string())
            .collect::<Vec<_>>()
    );
    assert!(
        outcome.invariant_violated(InvariantKind::Cleanup),
        "IVE Cleanup invariant must be Violated for double-free. \
         Verification result: {:?}",
        outcome.verification.as_ref().map(|v| &v.overall)
    );
    // The counterexample description should mention "double free" or
    // "released 2 time" (the IVE's actual phrasing).
    let desc = outcome.violation_description(InvariantKind::Cleanup);
    assert!(
        desc.as_ref().map(|d| {
            let lower = d.to_lowercase();
            lower.contains("double") || lower.contains("released") || lower.contains("2 time")
        }).unwrap_or(false),
        "cleanup counterexample description should mention double-free; got: {:?}",
        desc
    );
}

#[test]
fn test_double_free_compiles_without_verification() {
    // With verification off, the pipeline will happily compile a
    // double-freeing program (the codegen does not track ownership).
    let outcome = run_pipeline(DOUBLE_FREE_SOURCE, VerificationLevel::None);
    assert!(outcome.success, "double-free should compile with verification off");
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 5: Memory leak detection (IVE catches this)
// ═══════════════════════════════════════════════════════════════════════════

const LEAK_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(64);
        *buf = 42;
        val: i32 = *buf;
        return val;
    }
"#;

#[test]
fn test_memory_leak_parses_and_builds_scg() {
    let scg = parse_and_build_scg(LEAK_SOURCE).expect("leak program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    assert_eq!(
        count_deallocations(&scg),
        0,
        "expected 0 deallocations for a leak"
    );
}

#[test]
fn test_ive_detects_memory_leak() {
    // The IVE catches leaks via the Liveness invariant: when an
    // allocation has no matching deallocation, liveness reports
    // "Resource leak: memory ResN ... never deallocated".
    let outcome = run_pipeline(LEAK_SOURCE, VerificationLevel::Normal);
    assert!(
        outcome.stage_failed("ive-verification"),
        "leak must cause an IVE verification failure; got errors: {:?}",
        outcome
            .errors
            .iter()
            .map(|e| e.stage().to_string())
            .collect::<Vec<_>>()
    );
    assert!(
        outcome.invariant_violated(InvariantKind::Liveness),
        "IVE Liveness invariant must be Violated for a leak. \
         Verification result: {:?}",
        outcome.verification.as_ref().map(|v| &v.overall)
    );
    let desc = outcome.violation_description(InvariantKind::Liveness);
    assert!(
        desc.as_ref().map(|d| {
            let lower = d.to_lowercase();
            lower.contains("leak") || lower.contains("never deallocated")
        }).unwrap_or(false) ||
        outcome.invariant(InvariantKind::Liveness)
            .map(|p| p.result.message.to_lowercase().contains("leak") || p.result.message.to_lowercase().contains("violation"))
            .unwrap_or(false),
        "liveness violation should mention leak; desc={:?}",
        desc
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 6: Null pointer dereference
// ═══════════════════════════════════════════════════════════════════════════

const NULL_DEREF_SOURCE: &str = r#"
    fn main() -> i32 {
        val: i32 = *null;
        return val;
    }
"#;

#[test]
fn test_null_pointer_dereference_compiles_or_errors_cleanly() {
    // The IVE does not currently detect null dereference. The pipeline
    // must either compile the program (with verification off) or fail
    // with a *non-panic* error. Either outcome is acceptable; a panic
    // is not.
    let outcome = run_pipeline(NULL_DEREF_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success || !outcome.errors.is_empty(),
        "pipeline must return a definite outcome for null deref"
    );
    if outcome.success {
        assert!(outcome.binary_len > 0);
    }
}

#[test]
fn test_null_pointer_dereference_ive_known_gap() {
    // IVE does not currently detect null dereferences.
    let outcome = run_pipeline(NULL_DEREF_SOURCE, VerificationLevel::Normal);
    let any_violation_mentions_null = outcome
        .verification
        .as_ref()
        .map(|v| {
            v.per_invariant
                .iter()
                .any(|p| {
                    let msg = p.result.message.to_lowercase();
                    let desc = if let VerificationStatus::Violated { counterexample } = &p.result.status {
                        counterexample.description.to_lowercase()
                    } else {
                        String::new()
                    };
                    msg.contains("null") || desc.contains("null")
                })
        })
        .unwrap_or(false);
    assert!(
        !any_violation_mentions_null,
        "IVE does not currently flag null dereferences. If this fails, \
         the IVE has gained null-deref detection — update the module docs."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 7: Uninitialized memory read
// ═══════════════════════════════════════════════════════════════════════════

const UNINIT_READ_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(8);
        val: i32 = *buf;
        free(buf);
        return val;
    }
"#;

#[test]
fn test_uninitialized_read_parses_and_builds_scg() {
    let scg = parse_and_build_scg(UNINIT_READ_SOURCE)
        .expect("uninit-read program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    assert!(count_deallocations(&scg) >= 1);
    // Note: pure reads don't produce Access nodes — see module docs.
}

#[test]
fn test_uninitialized_read_compiles_without_verification() {
    let outcome = run_pipeline(UNINIT_READ_SOURCE, VerificationLevel::None);
    assert!(outcome.success, "uninit-read should compile with verification off");
    assert!(outcome.binary_len > 0);
}

#[test]
fn test_uninitialized_read_ive_known_gap() {
    // IVE does not currently detect uninitialized reads.
    let outcome = run_pipeline(UNINIT_READ_SOURCE, VerificationLevel::Normal);
    let any_violation_mentions_uninit = outcome
        .verification
        .as_ref()
        .map(|v| {
            v.per_invariant
                .iter()
                .any(|p| {
                    let msg = p.result.message.to_lowercase();
                    let desc = if let VerificationStatus::Violated { counterexample } = &p.result.status {
                        counterexample.description.to_lowercase()
                    } else {
                        String::new()
                    };
                    msg.contains("uninit") || desc.contains("uninit")
                })
        })
        .unwrap_or(false);
    assert!(
        !any_violation_mentions_uninit,
        "IVE does not currently flag uninitialized reads. If this fails, \
         the IVE has gained uninit detection — update the module docs."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 8: Valid programs must compile (no false-negative blocking)
// ═══════════════════════════════════════════════════════════════════════════

const VALID_PROGRAM_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(64);
        *buf = 42;
        val: i32 = *buf;
        free(buf);
        return val;
    }
"#;

#[test]
fn test_valid_program_parses_and_builds_scg() {
    let scg = parse_and_build_scg(VALID_PROGRAM_SOURCE)
        .expect("valid program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    assert!(count_deallocations(&scg) >= 1);
    // Note: only writes produce Access nodes in the current SCG
    // builder, so `*buf = 42` produces 1 access but `val = *buf`
    // produces 0. Don't assert a specific count.
}

#[test]
fn test_valid_program_compiles_without_verification() {
    // A clearly valid program (allocate, store, load, free, return)
    // must compile end-to-end when verification is off. This catches
    // regressions in the parser, SCG builder, IR lowering, regalloc,
    // and ELF emission.
    let outcome = run_pipeline(VALID_PROGRAM_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "valid program must compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0, "expected non-empty binary");
}

#[test]
fn test_valid_program_ive_false_positive_documented() {
    // The IVE currently produces a spurious "Resource leak" liveness
    // violation on programs that DO call `free`. Document this so that
    // a future IVE fix causes this test to fail (prompting removal of
    // the false-positive documentation).
    //
    // When the IVE stops producing this false positive, replace the
    // body of this test with:
    //   assert!(!outcome.has_violated_invariant(),
    //          "valid program should not fail IVE verification");
    let outcome = run_pipeline(VALID_PROGRAM_SOURCE, VerificationLevel::Normal);
    assert!(
        outcome.stage_failed("ive-verification"),
        "expected IVE to (currently spuriously) fail on a valid program; \
         if this assertion fails, the false positive has been fixed — \
         update the module docs and this test."
    );
    // The spurious violation should be Liveness, not Cleanup.
    assert!(
        outcome.invariant_violated(InvariantKind::Liveness),
        "spurious violation should be on Liveness invariant"
    );
    assert!(
        !outcome.invariant_violated(InvariantKind::Cleanup),
        "valid program must NOT violate Cleanup invariant"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 9: Multiple allocations + correct frees
// ═══════════════════════════════════════════════════════════════════════════

const MULTI_ALLOC_SOURCE: &str = r#"
    fn main() -> i32 {
        a = allocate(8);
        b = allocate(8);
        c = allocate(8);
        *a = 1;
        *b = 2;
        *c = 3;
        free(a);
        free(b);
        free(c);
        return 0;
    }
"#;

#[test]
fn test_multiple_allocs_correct_frees_parses_and_builds_scg() {
    let scg = parse_and_build_scg(MULTI_ALLOC_SOURCE)
        .expect("multi-alloc program must parse + build SCG");
    assert_eq!(count_allocations(&scg), 3, "expected 3 allocations");
    assert_eq!(count_deallocations(&scg), 3, "expected 3 deallocations");
}

#[test]
fn test_multiple_allocs_correct_frees_compiles() {
    let outcome = run_pipeline(MULTI_ALLOC_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "multi-alloc program with matching frees must compile; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 10: Nested function calls with memory
// ═══════════════════════════════════════════════════════════════════════════

const NESTED_CALLS_SOURCE: &str = r#"
    fn inner(p: i32) -> i32 {
        *p = 7;
        return 0;
    }
    fn outer() -> i32 {
        buf = allocate(8);
        inner(buf);
        val: i32 = *buf;
        free(buf);
        return val;
    }
    fn main() -> i32 {
        return outer();
    }
"#;

#[test]
fn test_nested_function_calls_with_memory_parses_and_builds_scg() {
    // The parser must accept functions that take Address parameters
    // and use them through dereference. (Note: full pipeline
    // compilation fails on this program because of an SCG→MSG cycle
    // in the call graph — see the module-level IVE notes.)
    let scg = parse_and_build_scg(NESTED_CALLS_SOURCE)
        .expect("nested-calls program must parse + build SCG");
    assert!(scg.node_count() > 0);
    // Note: allocations and deallocations inside outer() should still
    // be tracked, even though interprocedural CFG edges create cycles.
    assert!(count_allocations(&scg) >= 1);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 11: Pointer arithmetic within bounds
// ═══════════════════════════════════════════════════════════════════════════

const PTR_ARITH_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(16);
        *(buf + 4) = 99;
        val: i32 = *(buf + 4);
        free(buf);
        return val;
    }
"#;

#[test]
fn test_pointer_arithmetic_in_bounds_parses_and_builds_scg() {
    let scg = parse_and_build_scg(PTR_ARITH_SOURCE)
        .expect("pointer-arith program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // The write `*(buf + 4) = 99` produces an Access node; the read
    // `val = *(buf + 4)` currently does not. So expect exactly 1
    // access (the write).
    assert!(count_accesses(&scg) >= 1, "expected >=1 access (the write)");
}

#[test]
fn test_pointer_arithmetic_in_bounds_compiles() {
    let outcome = run_pipeline(PTR_ARITH_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "in-bounds pointer arithmetic must compile; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 12: Conditional free (if-else with free)
// ═══════════════════════════════════════════════════════════════════════════

const COND_FREE_BOTH_BRANCHES_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(64);
        x = 1;
        if x {
            free(buf);
        } else {
            free(buf);
        }
        return 0;
    }
"#;

const COND_FREE_ONE_BRANCH_SOURCE: &str = r#"
    fn main() -> i32 {
        buf = allocate(64);
        x = 1;
        if x {
            free(buf);
        }
        return 0;
    }
"#;

#[test]
fn test_conditional_free_both_branches_parses_and_compiles() {
    let scg = parse_and_build_scg(COND_FREE_BOTH_BRANCHES_SOURCE)
        .expect("cond-free-both program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // Both branches free, so the SCG should have at least one
    // deallocation node (control-flow merging may dedupe).
    assert!(count_deallocations(&scg) >= 1);

    let outcome = run_pipeline(COND_FREE_BOTH_BRANCHES_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "cond-free-both-branches must compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

#[test]
fn test_conditional_free_one_branch_parses_and_compiles() {
    let scg = parse_and_build_scg(COND_FREE_ONE_BRANCH_SOURCE)
        .expect("cond-free-one program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    assert!(count_deallocations(&scg) >= 1);

    let outcome = run_pipeline(COND_FREE_ONE_BRANCH_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "cond-free-one-branch must compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 13: Loop allocation (allocate/free inside a while body)
// ═══════════════════════════════════════════════════════════════════════════

const LOOP_ALLOC_SOURCE: &str = r#"
    fn main() -> i32 {
        i = 0;
        while i < 5 {
            buf = allocate(8);
            *buf = i;
            free(buf);
            i = i + 1;
        }
        return 0;
    }
"#;

#[test]
fn test_loop_alloc_free_parses_and_builds_scg() {
    // The parser must accept the `while` syntax and the SCG builder
    // must produce nodes for the alloc/store/load/free inside the loop
    // body. (Full pipeline compilation fails on this program because
    // of the SCG→MSG cycle detector — see module-level notes. The
    // parser+SCG path is the relevant correctness check.)
    let scg = parse_and_build_scg(LOOP_ALLOC_SOURCE)
        .expect("loop-alloc program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1, "expected >=1 allocation in loop body");
    assert!(count_deallocations(&scg) >= 1, "expected >=1 deallocation in loop body");
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 14: Function returns allocated memory (caller must free)
// ═══════════════════════════════════════════════════════════════════════════

const FN_RETURNS_ALLOC_SOURCE: &str = r#"
    fn make_buf() -> i32 {
        buf = allocate(64);
        *buf = 42;
        return buf;
    }
    fn main() -> i32 {
        b = make_buf();
        val: i32 = *b;
        free(b);
        return val;
    }
"#;

#[test]
fn test_function_returns_allocation_parses_and_builds_scg() {
    // The parser must accept a function that returns an Address-typed
    // value derived from `allocate`. (Full pipeline compilation fails
    // because of the SCG→MSG cycle detector on call graphs.)
    //
    // Note: the SCG builder does NOT track that `b` in main is the
    // same allocation as `buf` in make_buf, so `free(b)` does not
    // produce a Deallocation node. The assertion below only checks
    // that the SCG is non-empty and has an allocation.
    let scg = parse_and_build_scg(FN_RETURNS_ALLOC_SOURCE)
        .expect("fn-returns-alloc program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 15: Struct field access within bounds
// ═══════════════════════════════════════════════════════════════════════════

const STRUCT_FIELD_SOURCE: &str = r#"
    struct Point { x: i32, y: i32 }
    fn main() -> i32 {
        p = allocate(16);
        *(p + 0) = 10;
        *(p + 8) = 20;
        val: i32 = *(p + 0);
        free(p);
        return val;
    }
"#;

#[test]
fn test_struct_field_access_parses_and_builds_scg() {
    let scg = parse_and_build_scg(STRUCT_FIELD_SOURCE)
        .expect("struct-field program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    assert!(count_deallocations(&scg) >= 1);
    // Two writes (`*(p + 0) = 10; *(p + 8) = 20;`) should produce
    // Access nodes; the read (`val = *(p + 0);`) currently does not.
    // Expect >=2 accesses from the writes alone.
    assert!(count_accesses(&scg) >= 2, "expected >=2 accesses (2 writes)");
}

#[test]
fn test_struct_field_access_compiles() {
    let outcome = run_pipeline(STRUCT_FIELD_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "struct field access program must compile; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-cutting property: IVE must not panic on any input
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_ive_does_not_panic_on_variety_of_programs() {
    // Run the IVE on every program in this file. The IVE must produce
    // a definite (Pass/Fail/Inconclusive/NoChecks) verdict without
    // panicking, regardless of the program's content.
    let sources = [
        UAF_SOURCE,
        OVERFLOW_WRITE_SOURCE,
        OVERFLOW_READ_SOURCE,
        DOUBLE_FREE_SOURCE,
        LEAK_SOURCE,
        NULL_DEREF_SOURCE,
        UNINIT_READ_SOURCE,
        VALID_PROGRAM_SOURCE,
        MULTI_ALLOC_SOURCE,
        PTR_ARITH_SOURCE,
        COND_FREE_BOTH_BRANCHES_SOURCE,
        COND_FREE_ONE_BRANCH_SOURCE,
        STRUCT_FIELD_SOURCE,
    ];
    for (i, src) in sources.iter().enumerate() {
        let outcome = run_pipeline(src, VerificationLevel::Normal);
        // We must have reached a definite conclusion: either the
        // pipeline compiled (with or without verification result) or
        // it returned an error list. Either way, no panic.
        let _ = outcome; // just observe that we got here.
        let _ = i;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-cutting property: every test program parses
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_all_test_programs_parse_successfully() {
    // All test programs used in this file must be syntactically valid
    // VUMA — otherwise the higher-level property tests would be
    // testing parser failures, not the IVE / pipeline.
    let sources = [
        ("UAF_SOURCE", UAF_SOURCE),
        ("OVERFLOW_WRITE_SOURCE", OVERFLOW_WRITE_SOURCE),
        ("OVERFLOW_READ_SOURCE", OVERFLOW_READ_SOURCE),
        ("DOUBLE_FREE_SOURCE", DOUBLE_FREE_SOURCE),
        ("LEAK_SOURCE", LEAK_SOURCE),
        ("NULL_DEREF_SOURCE", NULL_DEREF_SOURCE),
        ("UNINIT_READ_SOURCE", UNINIT_READ_SOURCE),
        ("VALID_PROGRAM_SOURCE", VALID_PROGRAM_SOURCE),
        ("MULTI_ALLOC_SOURCE", MULTI_ALLOC_SOURCE),
        ("NESTED_CALLS_SOURCE", NESTED_CALLS_SOURCE),
        ("PTR_ARITH_SOURCE", PTR_ARITH_SOURCE),
        ("COND_FREE_BOTH_BRANCHES_SOURCE", COND_FREE_BOTH_BRANCHES_SOURCE),
        ("COND_FREE_ONE_BRANCH_SOURCE", COND_FREE_ONE_BRANCH_SOURCE),
        ("LOOP_ALLOC_SOURCE", LOOP_ALLOC_SOURCE),
        ("FN_RETURNS_ALLOC_SOURCE", FN_RETURNS_ALLOC_SOURCE),
        ("STRUCT_FIELD_SOURCE", STRUCT_FIELD_SOURCE),
    ];
    for (name, src) in sources {
        let result = parse_and_build_scg(src);
        assert!(
            result.is_ok(),
            "test program {} must parse + build SCG; got: {:?}",
            name,
            result.err()
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Property: IVE detection is consistent across re-runs (determinism)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_ive_double_free_detection_is_deterministic() {
    // The IVE must produce the same verdict across repeated runs on
    // the same input — non-determinism would indicate an internal
    // state leak or RNG dependency.
    let mut verdicts = Vec::new();
    for _ in 0..5 {
        let outcome = run_pipeline(DOUBLE_FREE_SOURCE, VerificationLevel::Normal);
        verdicts.push(outcome.invariant_violated(InvariantKind::Cleanup));
    }
    assert!(
        verdicts.iter().all(|&v| v),
        "double-free cleanup violation must be consistently detected across runs; got: {:?}",
        verdicts
    );
}

#[test]
fn test_ive_leak_detection_is_deterministic() {
    let mut verdicts = Vec::new();
    for _ in 0..5 {
        let outcome = run_pipeline(LEAK_SOURCE, VerificationLevel::Normal);
        verdicts.push(outcome.invariant_violated(InvariantKind::Liveness));
    }
    assert!(
        verdicts.iter().all(|&v| v),
        "leak liveness violation must be consistently detected across runs; got: {:?}",
        verdicts
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property: IVE verification is a no-op when VerificationLevel::None
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_verification_none_skips_ive() {
    // With VerificationLevel::None, the pipeline must not run the IVE
    // and therefore must not produce a verification result.
    let outcome = run_pipeline(VALID_PROGRAM_SOURCE, VerificationLevel::None);
    assert!(outcome.success, "valid program should compile with verification off");
    assert!(
        outcome.verification.is_none(),
        "no verification result should be produced when verification is None"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property: Every allocation node in the SCG has well-formed payload
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_allocation_nodes_have_allocation_payload() {
    // Structural invariant: every node with `node_type == Allocation`
    // must have a payload of `NodePayload::Allocation`. This catches
    // bugs in the SCG builder where node types and payloads get out
    // of sync.
    let scg = parse_and_build_scg(VALID_PROGRAM_SOURCE).expect("valid program must parse");
    for node in scg.nodes() {
        if node.node_type == NodeType::Allocation {
            assert!(
                matches!(node.payload, NodePayload::Allocation(_)),
                "Allocation node {:?} should have Allocation payload, got {:?}",
                node.id,
                node.payload
            );
        }
        if node.node_type == NodeType::Deallocation {
            assert!(
                matches!(node.payload, NodePayload::Deallocation(_)),
                "Deallocation node {:?} should have Deallocation payload, got {:?}",
                node.id,
                node.payload
            );
        }
    }
}
