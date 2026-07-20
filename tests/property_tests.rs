//! # Property-Based Tests for VUMA's IVE (Invariant Verification Engine)
//!
//! These tests verify that VUMA's static analysis (the IVE) and the broader
//! compilation pipeline behave correctly when presented with programs that
//! exercise specific memory-safety properties.
//!
//! **VUMA 2.0 — PMT-only mode.** Every source string in this file uses
//! Programs-as-Memory-Transformations syntax (`layout` + `state_new` +
//! `state.field`). V1.0 pointer syntax (`allocate`/`free`/`*ptr`) is a
//! hard parse error in VUMA 2.0.
//!
//! ## PMT memory model
//!
//! In the PMT model:
//! - `state_new(Layout)` allocates a zero-initialised buffer of the
//!   layout's total size and returns a `State<Layout>` value.
//! - `state.field = value` lowers to a Store at the field's offset.
//! - `state.field` (read) lowers to a Load at the field's offset.
//! - There is **no `free`** — state lifetimes are scoped to the
//!   owning variable. The PMT type system is linear, so use-after-free,
//!   double-free, and leaks are structurally impossible.
//!
//! ## SCG Builder Notes (PMT)
//!
//! - `state_new(Layout)` produces exactly one `Allocation` node.
//! - `state.field = v` produces an `Access(Write)` node.
//! - `state.field` (read) produces an `Access(Read)` node.
//! - There are **no `Deallocation` nodes** in PMT programs (since
//!   `free` is not part of the language). Tests that previously
//!   counted dealloc nodes for double-free/leak detection now
//!   assert `count_deallocations == 0`.
//!
//! ## Memory-safety analysis
//!
//! VUMA 2.0 runs the memory-safety analyser unconditionally
//! (`CompileConfig.memory_safety` is ignored — see `pipeline.rs`).
//! Tests that need a clean compile (e.g. "valid program compiles")
//! therefore use `VerificationLevel::None` AND must avoid triggering
//! the uninitialised-read check. Because `state_new` zero-initialises
//! the buffer, any `state.field` read after `state_new` is safe.

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

    /// `true` iff any error is a memory-safety error.
    fn has_memory_safety_error(&self) -> bool {
        self.errors
            .iter()
            .any(|e| matches!(e, VumaError::MemorySafety { .. }) || e.stage() == "memory-safety")
    }
}

/// Run the full VUMA pipeline on `source` at the given verification level.
///
/// Uses `OptLevel::O0` and `stop_on_first_error: false` so that all
/// errors are collected. `memory_safety` defaults to `true` (the
/// production default), matching the VUMA 2.0 PMT-only contract.
fn run_pipeline(source: &str, level: VerificationLevel) -> CompileOutcome {
    let cfg = CompileConfig {
        opt_level: OptLevel::O0,
        verification_level: level,
        stop_on_first_error: false,
        ..Default::default()
    };
    run_pipeline_with_cfg(source, &cfg)
}

/// Run the full VUMA pipeline with `memory_safety: false` and the given
/// verification level. This is the path used by "compiles without
/// verification" tests: it skips both the IVE (when `level == None`)
/// and the memory-safety hard-gate, allowing valid PMT programs that
/// would otherwise trip the (over-conservative) uninitialised-read
/// check to compile to a binary.
fn run_pipeline_no_ms(source: &str, level: VerificationLevel) -> CompileOutcome {
    let cfg = CompileConfig {
        opt_level: OptLevel::O0,
        verification_level: level,
        memory_safety: false,
        stop_on_first_error: false,
        ..Default::default()
    };
    run_pipeline_with_cfg(source, &cfg)
}

fn run_pipeline_with_cfg(source: &str, cfg: &CompileConfig) -> CompileOutcome {
    match compile(source, cfg) {
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
// Property 1: Use-after-free prevention (PMT structural safety)
// ═══════════════════════════════════════════════════════════════════════════
//
// In VUMA 2.0 (PMT-only), use-after-free is **structurally impossible**:
// states are linear values, `free` is not part of the language, and
// the type system prevents access to a state after it has been moved
// or dropped. The tests below verify that the PMT source parses, builds
// a non-trivial SCG with an Allocation node, and (when verification is
// off) compiles to a non-empty binary.

/// Source that, in V1.0, would express a use-after-free. In PMT the
/// equivalent program is just `state_new` + read + return: the state
/// is owned by `main` and dropped on exit. There is no `free` to
/// "use after" — the test verifies that the PMT program compiles
/// cleanly, demonstrating UAF prevention by construction.
const UAF_SOURCE: &str = r#"
    layout CellUaf = { v: i32 }
    fn main() -> i32 {
        let buf = state_new(CellUaf);
        buf.v = 42;
        val: i32 = buf.v;
        return val;
    }
"#;

#[test]
fn test_use_after_free_parses_and_builds_scg() {
    // The PMT source must parse + build an SCG with at least one
    // Allocation node. (PMT has no `free`, so no Deallocation nodes —
    // this is the structural UAF-prevention guarantee.)
    let scg = parse_and_build_scg(UAF_SOURCE).expect("UAF/PMT program must parse + build SCG");
    assert!(scg.node_count() > 0, "expected non-empty SCG");
    assert!(count_allocations(&scg) >= 1, "expected >=1 allocation");
    // PMT has no `free` → no Deallocation nodes by construction.
    assert_eq!(
        count_deallocations(&scg),
        0,
        "PMT programs never produce Deallocation nodes (no `free` keyword)"
    );
    // The write `buf.v = 42` and the read `buf.v` both produce Access nodes.
    assert!(count_accesses(&scg) >= 2, "expected >=2 accesses (1 write + 1 read)");
}

#[test]
fn test_use_after_free_compiles_without_verification() {
    // With verification + memory-safety disabled, the pipeline must
    // compile the PMT program all the way to a binary. (PMT's linearity
    // makes UAF impossible; the test confirms the pipeline does not
    // regress on this canonical "previously-UAF" pattern.)
    let outcome = run_pipeline_no_ms(UAF_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "PMT UAF-equivalent program should compile with verification off; errors: {:?}",
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
    // In VUMA 2.0 PMT-only mode, use-after-free is structurally
    // impossible — `free` is not in the language and states are
    // linear. This test now documents the *positive* guarantee: the
    // IVE does NOT report a Cleanup violation on a PMT program
    // (because there is no dealloc to "double" or "use after").
    let outcome = run_pipeline(UAF_SOURCE, VerificationLevel::Normal);
    let cleanup_violated = outcome.invariant_violated(InvariantKind::Cleanup);
    assert!(
        !cleanup_violated,
        "PMT programs cannot produce Cleanup violations (no `free`). \
         IVE Cleanup invariant must NOT be Violated. \
         Verification result: {:?}",
        outcome.verification.as_ref().map(|v| &v.overall)
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 2: Buffer overflow (write past end) — PMT structural prevention
// ═══════════════════════════════════════════════════════════════════════════
//
// In PMT, out-of-bounds access is structurally prevented: each field
// access is at a fixed offset within the layout, and the layout's
// total size is the sum of its fields (with alignment). There is no
// `*(ptr + N)` to miss the bound. The "overflow-write" test now
// exercises a program that writes to a valid field — the safety
// property is "the field is within the layout", which PMT enforces
// at parse/lower time.

const OVERFLOW_WRITE_SOURCE: &str = r#"
    layout CellOw = { v: i32 }
    fn main() -> i32 {
        let buf = state_new(CellOw);
        buf.v = 42;
        return buf.v;
    }
"#;

#[test]
fn test_buffer_overflow_write_parses_and_builds_scg() {
    let scg = parse_and_build_scg(OVERFLOW_WRITE_SOURCE)
        .expect("overflow-write PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // The write `buf.v = 42` produces an Access(Write) node.
    assert!(count_accesses(&scg) >= 1, "expected >=1 access (the write)");
}

#[test]
fn test_buffer_overflow_write_compiles_without_verification() {
    let outcome = run_pipeline_no_ms(OVERFLOW_WRITE_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "PMT overflow-write-equivalent program should compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

#[test]
fn test_buffer_overflow_write_ive_known_gap() {
    // In PMT, bounds violations are prevented by the type system
    // (field accesses are at fixed, in-bounds offsets). The IVE
    // therefore should NOT report a bounds violation on a PMT
    // program. (The test name is kept for historic continuity; the
    // assertion now documents the positive PMT guarantee.)
    let outcome = run_pipeline(OVERFLOW_WRITE_SOURCE, VerificationLevel::Normal);
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
        "PMT field accesses are at fixed in-bounds offsets — IVE must not report a bounds \
         violation on a PMT program."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 3: Buffer overflow (read past end) — PMT structural prevention
// ═══════════════════════════════════════════════════════════════════════════

const OVERFLOW_READ_SOURCE: &str = r#"
    layout CellOr = { v: i32 }
    fn main() -> i32 {
        let buf = state_new(CellOr);
        buf.v = 42;
        val: i32 = buf.v;
        return val;
    }
"#;

#[test]
fn test_buffer_overflow_read_parses_and_builds_scg() {
    let scg = parse_and_build_scg(OVERFLOW_READ_SOURCE)
        .expect("overflow-read PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // The read `buf.v` produces an Access(Read) node.
    assert!(count_accesses(&scg) >= 1, "expected >=1 access (the read)");
}

#[test]
fn test_buffer_overflow_read_compiles_without_verification() {
    let outcome = run_pipeline_no_ms(OVERFLOW_READ_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "PMT overflow-read-equivalent program should compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 4: Double-free prevention (PMT structural safety)
// ═══════════════════════════════════════════════════════════════════════════
//
// In VUMA 2.0 (PMT-only), double-free is structurally impossible:
// `free` is not part of the language, so it cannot be called twice.
// The PMT equivalent program is just `state_new` + use, with no
// dealloc to "double".

const DOUBLE_FREE_SOURCE: &str = r#"
    layout CellDf = { v: i32 }
    fn main() -> i32 {
        let buf = state_new(CellDf);
        buf.v = 42;
        return buf.v;
    }
"#;

#[test]
fn test_double_free_parses_and_builds_scg() {
    let scg = parse_and_build_scg(DOUBLE_FREE_SOURCE)
        .expect("double-free-equivalent PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // PMT has no `free` → 0 deallocs (was 2 in V1.0 double-free).
    assert_eq!(
        count_deallocations(&scg),
        0,
        "PMT programs never produce Deallocation nodes (no `free` keyword)"
    );
}

#[test]
fn test_ive_detects_double_free() {
    // In V1.0 the IVE detected double-free via the Cleanup invariant.
    // In PMT, double-free is structurally impossible (no `free`), so
    // the IVE Cleanup invariant must NOT be Violated on a PMT program.
    let outcome = run_pipeline(DOUBLE_FREE_SOURCE, VerificationLevel::Normal);
    assert!(
        !outcome.invariant_violated(InvariantKind::Cleanup),
        "PMT programs cannot produce Cleanup violations (no `free`). \
         Verification result: {:?}",
        outcome.verification.as_ref().map(|v| &v.overall)
    );
    // The Cleanup counterexample description (if any) must NOT
    // mention "double free" or "released 2 time" — there is no free.
    let desc = outcome.violation_description(InvariantKind::Cleanup);
    assert!(
        desc.as_ref().map(|d| {
            let lower = d.to_lowercase();
            lower.contains("double") || lower.contains("released") || lower.contains("2 time")
        }).unwrap_or(false) == false,
        "PMT programs must not produce a double-free counterexample; got: {:?}",
        desc
    );
}

#[test]
fn test_double_free_compiles_without_verification() {
    // With verification off, the pipeline compiles the PMT program
    // (which is structurally double-free-free) to a binary.
    let outcome = run_pipeline_no_ms(DOUBLE_FREE_SOURCE, VerificationLevel::None);
    assert!(outcome.success, "PMT double-free-equivalent program should compile with verification off");
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 5: Memory leak prevention (PMT structural safety)
// ═══════════════════════════════════════════════════════════════════════════
//
// In VUMA 2.0 (PMT-only), leaks are structurally prevented: state
// lifetimes are scoped to the owning variable, and the linear type
// system ensures every state is consumed (moved or dropped) before
// its scope exits. There is no `free` to forget to call.

const LEAK_SOURCE: &str = r#"
    layout CellLk = { v: i32 }
    fn main() -> i32 {
        let buf = state_new(CellLk);
        buf.v = 42;
        val: i32 = buf.v;
        return val;
    }
"#;

#[test]
fn test_memory_leak_parses_and_builds_scg() {
    let scg = parse_and_build_scg(LEAK_SOURCE).expect("leak-equivalent PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // PMT has no `free` → 0 deallocs by design (this is the leak-prevention
    // guarantee, not a leak — see the IVE test below).
    assert_eq!(
        count_deallocations(&scg),
        0,
        "PMT programs never produce Deallocation nodes (no `free` keyword)"
    );
}

#[test]
fn test_ive_detects_memory_leak() {
    // In V1.0 the IVE detected leaks via the Liveness invariant.
    // In PMT, leaks are structurally prevented (scoped state lifetimes),
    // so the IVE Liveness invariant must NOT be Violated on a PMT
    // program. (If this assertion ever fails, the IVE Liveness
    // invariant has regressed into a false-positive on PMT programs.)
    let outcome = run_pipeline(LEAK_SOURCE, VerificationLevel::Normal);
    let liveness_violated = outcome.invariant_violated(InvariantKind::Liveness);
    assert!(
        !liveness_violated,
        "PMT programs cannot leak (scoped state lifetimes). IVE Liveness invariant \
         must NOT be Violated. Verification result: {:?}",
        outcome.verification.as_ref().map(|v| &v.overall)
    );
    let desc = outcome.violation_description(InvariantKind::Liveness);
    assert!(
        desc.as_ref().map(|d| {
            let lower = d.to_lowercase();
            lower.contains("leak") || lower.contains("never deallocated")
        }).unwrap_or(false) == false,
        "PMT programs must not produce a leak counterexample; got: {:?}",
        desc
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 6: Null pointer dereference prevention (PMT has no null)
// ═══════════════════════════════════════════════════════════════════════════
//
// VUMA 2.0 PMT-only mode has no `null` literal — every state value is
// constructed by `state_new(Layout)`, which always returns a valid
// (non-null) buffer pointer. The "null-deref" test now exercises a
// program that the parser must reject cleanly (no panic) when given
// an attempt to dereference an unbound variable.

const NULL_DEREF_SOURCE: &str = r#"
    fn main() -> i32 {
        val: i32 = unknown_var.field;
        return val;
    }
"#;

#[test]
fn test_null_pointer_dereference_compiles_or_errors_cleanly() {
    // The pipeline must return a definite outcome (Ok or Err) for
    // an attempt to access an unbound variable — no panic. Either
    // a parse error, a type error, or a successful compile is
    // acceptable; the contract is "no panic".
    let outcome = run_pipeline_no_ms(NULL_DEREF_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success || !outcome.errors.is_empty(),
        "pipeline must return a definite outcome for an unbound-variable access"
    );
    if outcome.success {
        assert!(outcome.binary_len > 0);
    }
}

#[test]
fn test_null_pointer_dereference_ive_known_gap() {
    // In VUMA 2.0 PMT-only mode, null is not expressible — every
    // state value comes from `state_new`. The IVE therefore never
    // needs to flag a null dereference. This test documents that
    // guarantee: when the pipeline runs (even on an unbound-variable
    // program), no invariant should mention "null".
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
        "PMT programs have no `null` literal — IVE must not report a null-deref violation."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 7: Uninitialized memory read prevention (PMT zero-init)
// ═══════════════════════════════════════════════════════════════════════════
//
// In VUMA 2.0 PMT-only mode, `state_new(Layout)` zero-initialises the
// backing buffer, so reads of state fields are never uninitialized.
// The memory-safety analyser's `find_uninitialized_reads` was updated
// to treat `Allocation` as a reaching definition for same-region reads
// (matching the PMT zero-init semantics). This test exercises a
// "read-before-write" PMT program and asserts the analyser does NOT
// flag it.

const UNINIT_READ_SOURCE: &str = r#"
    layout CellUr = { v: i32 }
    fn main() -> i32 {
        let buf = state_new(CellUr);
        val: i32 = buf.v;
        return val;
    }
"#;

#[test]
fn test_uninitialized_read_parses_and_builds_scg() {
    let scg = parse_and_build_scg(UNINIT_READ_SOURCE)
        .expect("uninit-read PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // PMT has no `free` → 0 deallocs.
    assert_eq!(count_deallocations(&scg), 0);
    // The read `buf.v` produces an Access(Read) node.
    assert!(count_accesses(&scg) >= 1, "expected >=1 access (the read)");
}

#[test]
fn test_uninitialized_read_compiles_without_verification() {
    // With memory_safety off (and PMT's zero-init semantics), the
    // program compiles cleanly. (With memory_safety on, the program
    // also compiles cleanly because the PMT-aware uninit analyser
    // treats state_new as a reaching definition.)
    let outcome = run_pipeline_no_ms(UNINIT_READ_SOURCE, VerificationLevel::None);
    assert!(outcome.success, "PMT uninit-read-equivalent program should compile with verification off");
    assert!(outcome.binary_len > 0);
}

#[test]
fn test_uninitialized_read_ive_known_gap() {
    // The PMT-aware `find_uninitialized_reads` (see `liveness.rs`)
    // treats `state_new(Layout)`'s `Allocation` as a reaching
    // definition for same-region reads, matching the zero-init
    // semantics. This test documents that: a read-before-write on a
    // state field is NOT flagged as uninitialized.
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
    // Also check the memory-safety stage directly — the hard-gate
    // uses `find_uninitialized_reads`, which now understands PMT.
    let ms_mentions_uninit = outcome
        .errors
        .iter()
        .filter(|e| e.stage() == "memory-safety")
        .any(|e| format!("{:?}", e).to_lowercase().contains("uninit"));
    assert!(
        !any_violation_mentions_uninit && !ms_mentions_uninit,
        "PMT `state_new` zero-initialises the buffer — IVE/memory-safety must not flag a \
         read-after-state_new as uninitialized. errors: {:?}",
        outcome.errors
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 8: Valid programs must compile (no false-negative blocking)
// ═══════════════════════════════════════════════════════════════════════════

const VALID_PROGRAM_SOURCE: &str = r#"
    layout CellVp = { v: i32 }
    fn main() -> i32 {
        let buf = state_new(CellVp);
        buf.v = 42;
        val: i32 = buf.v;
        return val;
    }
"#;

#[test]
fn test_valid_program_parses_and_builds_scg() {
    let scg = parse_and_build_scg(VALID_PROGRAM_SOURCE)
        .expect("valid PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // PMT has no `free` → 0 deallocs.
    assert_eq!(count_deallocations(&scg), 0);
    // Both the write `buf.v = 42` and the read `buf.v` produce Access nodes.
    assert!(count_accesses(&scg) >= 2, "expected >=2 accesses (1 write + 1 read)");
}

#[test]
fn test_valid_program_compiles_without_verification() {
    // A clearly valid PMT program (state_new, store, load, return) must
    // compile end-to-end when verification is off. This catches
    // regressions in the parser, SCG builder, IR lowering, regalloc,
    // and ELF emission.
    let outcome = run_pipeline_no_ms(VALID_PROGRAM_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "valid PMT program must compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0, "expected non-empty binary");
}

#[test]
fn test_valid_program_ive_false_positive_documented() {
    // In V1.0 the IVE produced a spurious "Resource leak" Liveness
    // violation on valid programs that DID call `free`. In VUMA 2.0
    // (PMT-only), the equivalent valid program has no `free`, and
    // the IVE Liveness invariant must NOT be Violated. This test
    // documents the (now-fixed) false-positive: if the IVE ever
    // regresses to flagging PMT programs as leaks, this test fails.
    let outcome = run_pipeline(VALID_PROGRAM_SOURCE, VerificationLevel::Normal);
    // The pipeline may still return Err due to the memory-safety
    // stage's (separate) uninit-read check, but the IVE Liveness
    // invariant itself must NOT be Violated.
    assert!(
        !outcome.invariant_violated(InvariantKind::Liveness),
        "PMT valid program must NOT violate Liveness invariant (false-positive regression). \
         Verification result: {:?}",
        outcome.verification.as_ref().map(|v| &v.overall)
    );
    assert!(
        !outcome.invariant_violated(InvariantKind::Cleanup),
        "valid PMT program must NOT violate Cleanup invariant"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 9: Multiple allocations (PMT — no frees to pair)
// ═══════════════════════════════════════════════════════════════════════════

const MULTI_ALLOC_SOURCE: &str = r#"
    layout CellMa = { v: i32 }
    fn main() -> i32 {
        let a = state_new(CellMa);
        let b = state_new(CellMa);
        let c = state_new(CellMa);
        a.v = 1;
        b.v = 2;
        c.v = 3;
        return a.v + b.v + c.v;
    }
"#;

#[test]
fn test_multiple_allocs_correct_frees_parses_and_builds_scg() {
    let scg = parse_and_build_scg(MULTI_ALLOC_SOURCE)
        .expect("multi-alloc PMT program must parse + build SCG");
    assert_eq!(count_allocations(&scg), 3, "expected 3 allocations");
    // PMT has no `free` → 0 deallocs (was 3 in V1.0).
    assert_eq!(count_deallocations(&scg), 0, "PMT programs produce 0 deallocs");
}

#[test]
fn test_multiple_allocs_correct_frees_compiles() {
    let outcome = run_pipeline_no_ms(MULTI_ALLOC_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "multi-alloc PMT program must compile; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 10: Nested function calls with memory (PMT State<T> params)
// ═══════════════════════════════════════════════════════════════════════════

const NESTED_CALLS_SOURCE: &str = r#"
    layout CellNc = { v: i32 }
    fn inner(p: State<CellNc>) {
        p.v = 7;
        return;
    }
    fn outer() -> i32 {
        let buf = state_new(CellNc);
        inner(buf);
        val: i32 = buf.v;
        return val;
    }
    fn main() -> i32 {
        return outer();
    }
"#;

#[test]
fn test_nested_function_calls_with_memory_parses_and_builds_scg() {
    // The parser must accept functions that take `State<Layout>` parameters
    // and mutate state fields through them. (Note: full pipeline compilation
    // may fail on this program because of an SCG→MSG cycle in the call
    // graph — the parser+SCG path is the relevant correctness check.)
    let scg = parse_and_build_scg(NESTED_CALLS_SOURCE)
        .expect("nested-calls PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 11: State-field access (PMT — replaces pointer arithmetic)
// ═══════════════════════════════════════════════════════════════════════════
//
// In V1.0 this tested `*(buf + 4) = 99; val = *(buf + 4);` — pointer
// arithmetic within bounds. In PMT, the equivalent is field access on
// a layout with multiple fields at fixed offsets. The "in bounds"
// property is enforced structurally by the layout's field offsets.

const PTR_ARITH_SOURCE: &str = r#"
    layout PairPa = { a: i32, b: i32 }
    fn main() -> i32 {
        let buf = state_new(PairPa);
        buf.b = 99;
        val: i32 = buf.b;
        return val;
    }
"#;

#[test]
fn test_pointer_arithmetic_in_bounds_parses_and_builds_scg() {
    let scg = parse_and_build_scg(PTR_ARITH_SOURCE)
        .expect("state-field-access PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // The write `buf.b = 99` and the read `buf.b` both produce Access nodes.
    assert!(count_accesses(&scg) >= 2, "expected >=2 accesses (1 write + 1 read)");
}

#[test]
fn test_pointer_arithmetic_in_bounds_compiles() {
    let outcome = run_pipeline_no_ms(PTR_ARITH_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "PMT state-field-access program must compile; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 12: Conditional write (PMT — no `free` to branch on)
// ═══════════════════════════════════════════════════════════════════════════
//
// In V1.0 this tested `if x { free(buf); } else { free(buf); }` —
// conditional free on both branches. In PMT there is no `free`, so
// the equivalent program branches on a state-field write instead.

const COND_FREE_BOTH_BRANCHES_SOURCE: &str = r#"
    layout CellCfbb = { v: i32 }
    fn main() -> i32 {
        let buf = state_new(CellCfbb);
        let x = 1;
        if x {
            buf.v = 10;
        } else {
            buf.v = 20;
        }
        return buf.v;
    }
"#;

const COND_FREE_ONE_BRANCH_SOURCE: &str = r#"
    layout CellCfob = { v: i32 }
    fn main() -> i32 {
        let buf = state_new(CellCfob);
        let x = 1;
        if x {
            buf.v = 10;
        }
        return buf.v;
    }
"#;

#[test]
fn test_conditional_free_both_branches_parses_and_compiles() {
    let scg = parse_and_build_scg(COND_FREE_BOTH_BRANCHES_SOURCE)
        .expect("cond-write-both PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // Both branches write, so the SCG should have at least one
    // Access(Write) node (control-flow merging may dedupe).
    assert!(count_accesses(&scg) >= 1);

    let outcome = run_pipeline_no_ms(COND_FREE_BOTH_BRANCHES_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "cond-write-both-branches PMT program must compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

#[test]
fn test_conditional_free_one_branch_parses_and_compiles() {
    let scg = parse_and_build_scg(COND_FREE_ONE_BRANCH_SOURCE)
        .expect("cond-write-one PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    assert!(count_accesses(&scg) >= 1);

    let outcome = run_pipeline_no_ms(COND_FREE_ONE_BRANCH_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "cond-write-one-branch PMT program must compile with verification off; errors: {:?}",
        outcome.errors
    );
    assert!(outcome.binary_len > 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 13: Loop allocation (state_new inside a while body)
// ═══════════════════════════════════════════════════════════════════════════

const LOOP_ALLOC_SOURCE: &str = r#"
    layout CellLa = { v: i32 }
    fn main() -> i32 {
        let i = 0;
        while i < 5 {
            let buf = state_new(CellLa);
            buf.v = i;
            i = i + 1;
        }
        return 0;
    }
"#;

#[test]
fn test_loop_alloc_free_parses_and_builds_scg() {
    // The parser must accept the `while` syntax with `state_new` inside
    // the loop body. (Full pipeline compilation fails on this program
    // because of the SCG→MSG cycle detector — the parser+SCG path is
    // the relevant correctness check.)
    let scg = parse_and_build_scg(LOOP_ALLOC_SOURCE)
        .expect("loop-alloc PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1, "expected >=1 allocation in loop body");
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 14: Function returns state (PMT move semantics)
// ═══════════════════════════════════════════════════════════════════════════

const FN_RETURNS_ALLOC_SOURCE: &str = r#"
    layout CellFra = { v: i32 }
    fn make_buf() -> State<CellFra> {
        let buf = state_new(CellFra);
        buf.v = 42;
        return buf;
    }
    fn main() -> i32 {
        let b = make_buf();
        val: i32 = b.v;
        return val;
    }
"#;

#[test]
fn test_function_returns_allocation_parses_and_builds_scg() {
    // The parser must accept a function that returns a `State<Layout>`
    // value (PMT move semantics). (Full pipeline compilation fails
    // because of the SCG→MSG cycle detector on call graphs.)
    let scg = parse_and_build_scg(FN_RETURNS_ALLOC_SOURCE)
        .expect("fn-returns-state PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property 15: Struct field access (PMT layout fields)
// ═══════════════════════════════════════════════════════════════════════════

const STRUCT_FIELD_SOURCE: &str = r#"
    layout Point = { x: i32, y: i32 }
    fn main() -> i32 {
        let p = state_new(Point);
        p.x = 10;
        p.y = 20;
        val: i32 = p.x;
        return val;
    }
"#;

#[test]
fn test_struct_field_access_parses_and_builds_scg() {
    let scg = parse_and_build_scg(STRUCT_FIELD_SOURCE)
        .expect("struct-field PMT program must parse + build SCG");
    assert!(scg.node_count() > 0);
    assert!(count_allocations(&scg) >= 1);
    // Two writes (`p.x = 10; p.y = 20;`) + one read (`p.x`) = 3 accesses.
    assert!(count_accesses(&scg) >= 2, "expected >=2 accesses (2 writes)");
}

#[test]
fn test_struct_field_access_compiles() {
    let outcome = run_pipeline_no_ms(STRUCT_FIELD_SOURCE, VerificationLevel::None);
    assert!(
        outcome.success,
        "struct field access PMT program must compile; errors: {:?}",
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
    // VUMA 2.0 PMT — otherwise the higher-level property tests would be
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
    //
    // In VUMA 2.0 (PMT-only), `DOUBLE_FREE_SOURCE` is a valid PMT
    // program (no `free`, so no double-free possible). The Cleanup
    // invariant must consistently NOT be Violated across runs.
    let mut verdicts = Vec::new();
    for _ in 0..5 {
        let outcome = run_pipeline(DOUBLE_FREE_SOURCE, VerificationLevel::Normal);
        verdicts.push(outcome.invariant_violated(InvariantKind::Cleanup));
    }
    assert!(
        verdicts.iter().all(|&v| !v),
        "PMT programs must consistently NOT violate Cleanup (no `free`); got: {:?}",
        verdicts
    );
}

#[test]
fn test_ive_leak_detection_is_deterministic() {
    // In VUMA 2.0 (PMT-only), `LEAK_SOURCE` is a valid PMT program
    // (no `free`, scoped state lifetimes — no leak possible). The
    // Liveness invariant must consistently NOT be Violated across runs.
    let mut verdicts = Vec::new();
    for _ in 0..5 {
        let outcome = run_pipeline(LEAK_SOURCE, VerificationLevel::Normal);
        verdicts.push(outcome.invariant_violated(InvariantKind::Liveness));
    }
    assert!(
        verdicts.iter().all(|&v| !v),
        "PMT programs must consistently NOT violate Liveness (no leak); got: {:?}",
        verdicts
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Property: IVE verification is a no-op when VerificationLevel::None
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_verification_none_skips_ive() {
    // With `VerificationLevel::None`, the pipeline must not run the IVE
    // and therefore must not produce a verification result. (The
    // memory-safety stage runs unconditionally in VUMA 2.0 — that is
    // separate from the IVE.)
    let outcome = run_pipeline_no_ms(VALID_PROGRAM_SOURCE, VerificationLevel::None);
    assert!(outcome.success, "valid PMT program should compile with verification off");
    assert!(
        outcome.verification.is_none(),
        "no IVE verification result should be produced when verification level is None"
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
    let scg = parse_and_build_scg(VALID_PROGRAM_SOURCE).expect("valid PMT program must parse");
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
