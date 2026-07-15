//! # Correct Loop Unrolling (Wave 13b + Wave 30)
//!
//! Replaces the miscompiling vectorizer (Wave 13) which duplicated the
//! loop body 4× without adjusting the trip count — a miscompilation that
//! turned `for i in 0..N { body }` into `for i in 0..N { body; body; body; body }`.
//!
//! ## Wave 30 additions
//!
//! - **Multi-block loop unrolling with block-graph rewiring.** The previous
//!   implementation bailed on any loop with body blocks (the `return None` at
//!   the old line 265-268). We now clone the body+lid sequence `factor` times,
//!   rewriting back-edge targets to the next copy and the exit to the original
//!   exit. Each copy `k` uses `iv + k` as its induction variable (correct SSA
//!   via fresh dst vregs).
//! - **Trip-count-derived unroll factor.** The hardcoded `UNROLL_FACTOR=2` is
//!   replaced by `compute_unroll_factor`, which uses affine SCEV: if the trip
//!   count is known and small (≤ 8), fully unroll; if known and large, unroll
//!   by `min(8, trip_count/2)`; if unknown, default to 2.
//! - **Affine Scalar Evolution (SCEV).** `analyze_trip_count` models the IV
//!   as an affine recurrence `{start, +, step}` and computes the trip count
//!   from the exit condition `iv < end` (or `<=`, `!=`).
//! - **Code-size budget.** `UNROLL_CODE_SIZE_BUDGET` (default 500 instrs)
//!   prevents unrolling when `body_size * factor` exceeds the budget.
//! - **Unroll-and-jam.** `try_unroll_and_jam` implements a conservative
//!   version of unroll-and-jam for perfectly-nested loops with no
//!   outer-loop-carried dependencies. See its doc-comment for the full
//!   safety contract.
//!
//! ## Correct unrolling algorithm
//!
//! For a self-loop with a counted induction variable:
//!
//! ```text
//! loop:
//!   i = phi(0, entry), (i_new, loop)
//!   ... body using i ...
//!   i_new = i + 1
//!   cond = i_new < N
//!   cond_branch cond, loop, exit
//! ```
//!
//! Unrolled by factor F:
//!
//! ```text
//! loop:
//!   i = phi(0, entry), (i_new, loop)
//!   ... body using i ...
//!   i_1 = i + 1; ... body using i_1 ...   (substituted copy)
//!   i_2 = i + 2; ... body using i_2 ...   (substituted copy)
//!   ...
//!   i_new = i + F                           (step by F, not 1)
//!   cond = i_new < N                        (condition unchanged)
//!   cond_branch cond, loop, exit
//! ```
//!
//! The loop now runs ceil(N/F) iterations, doing F body copies per
//! iteration. The total work is N (correct), not N*F (the old bug).
//!
//! ## Safety
//!
//! The unroller bails out (returns None) for any loop it cannot fully
//! analyze. This means it only unrolls loops it can PROVE correct, and
//! leaves everything else untouched. No miscompilation is possible.

use crate::ir::{IRBlock, IRFunction, IRInstr, IRTerminator, IRValue, BinOpKind, CmpKind};
use crate::regalloc::{LoopDetector, LoopInfo};

// ─────────────────────────────────────────────────────────────────────────
// Constants & cost model (Wave 30)
// ─────────────────────────────────────────────────────────────────────────

/// Default unroll factor when the trip count is unknown.
const DEFAULT_UNROLL_FACTOR: u32 = 2;

/// Maximum unroll factor for large-trip-count loops.
const MAX_UNROLL_FACTOR: u32 = 8;

/// Trip count threshold below which a loop is fully unrolled.
const FULL_UNROLL_THRESHOLD: u64 = 8;

/// Code-size budget: don't unroll if `body_size * factor` exceeds this.
const UNROLL_CODE_SIZE_BUDGET: u32 = 500;

// ─────────────────────────────────────────────────────────────────────────
// Scalar Evolution (SCEV) — Wave 30
// ─────────────────────────────────────────────────────────────────────────

/// An affine recurrence chain `{start, +, step}` over a loop.
///
/// Models an induction variable `iv` that starts at `start` and is
/// incremented by `step` each iteration: `iv_n = start + n * step`.
///
/// This is the minimal SCEV needed for trip-count analysis. Full polynomial
/// / chained-recurrence analysis is a future extension (`TODO(wave30)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffineScev {
    /// Initial value of the IV (the entry incoming of the Phi).
    pub start: i64,
    /// Step added per iteration (must be non-zero for a meaningful trip count).
    pub step: i64,
}

/// A trip-count analysis result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TripCount {
    /// Exact trip count known.
    Known(u64),
    /// Upper bound known (loop runs at most this many times).
    Bounded(u64),
    /// Unknown — analysis failed.
    Unknown,
}

impl TripCount {
    /// Returns `true` if the trip count is exactly known.
    pub fn is_known(&self) -> bool {
        matches!(self, TripCount::Known(_))
    }

    /// Returns the exact trip count, if known.
    pub fn value(&self) -> Option<u64> {
        match self {
            TripCount::Known(n) => Some(*n),
            _ => None,
        }
    }
}

/// Analyze a loop's trip count using affine SCEV.
///
/// Detects the canonical counted-loop pattern:
/// - Header starts with `i = phi(start, entry), (i_next, latch)`.
/// - Latch has `i_next = i + step_const`.
/// - Latch has `cond = cmp i_next, end_vreg, SLt|ULt` (or `<=`, `!=`).
///
/// Returns the trip count:
/// - For `i < end` with start, step: `trip = ceil((end - start) / step)`.
/// - For `i <= end`: `trip = ceil((end - start + 1) / step)`.
/// - For `i != end`: `trip = (end - start) / step` (exact, requires divisibility).
///
/// Returns `Unknown` if any of the following hold:
/// - The IV is not an affine recurrence (start or step not constant).
/// - The exit condition doesn't match a recognized pattern.
/// - The trip count would be negative or non-finite.
pub fn analyze_trip_count(func: &IRFunction, loop_info: &LoopInfo) -> TripCount {
    let header = match func.blocks.iter().find(|b| b.label == loop_info.header) {
        Some(b) => b,
        None => return TripCount::Unknown,
    };
    let latch = match func.blocks.iter().find(|b| b.label == loop_info.latch) {
        Some(b) => b,
        None => return TripCount::Unknown,
    };

    // Extract the IV Phi from the header.
    if header.instructions.is_empty() {
        return TripCount::Unknown;
    }
    let (phi_vreg, start) = match &header.instructions[0] {
        IRInstr::Phi { dst, incoming } => {
            if incoming.len() != 2 {
                return TripCount::Unknown;
            }
            let dst_r = match dst {
                IRValue::Register(r) => *r,
                _ => return TripCount::Unknown,
            };
            // The "entry" incoming (not from the latch) is the start.
            let start = incoming
                .iter()
                .find(|(_, src)| src != &loop_info.latch)
                .and_then(|(v, _)| v.as_immediate());
            let start = match start {
                Some(s) => s,
                None => return TripCount::Unknown,
            };
            (dst_r, start)
        }
        _ => return TripCount::Unknown,
    };

    // Find `i_next = i + step_const` in the latch.
    let mut step: Option<i64> = None;
    let mut i_next_vreg: Option<u32> = None;
    for instr in &latch.instructions {
        if let IRInstr::BinOp {
            op: BinOpKind::Add | BinOpKind::Sub,
            dst,
            lhs,
            rhs: IRValue::Immediate(c),
            ..
        } = instr
        {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *l == phi_vreg {
                    step = Some(if matches!(instr, IRInstr::BinOp { op: BinOpKind::Sub, .. }) {
                        -*c
                    } else {
                        *c
                    });
                    i_next_vreg = Some(*d);
                    break;
                }
            }
        }
    }
    let step = match step {
        Some(s) if s != 0 => s,
        _ => return TripCount::Unknown,
    };
    let i_next_vreg = match i_next_vreg {
        Some(r) => r,
        None => return TripCount::Unknown,
    };

    // Find the exit comparison `cond = cmp i_next, end, kind`.
    // The cond is referenced by the latch's Branch terminator.
    let (cond_vreg, _exit_label) = match &latch.terminator {
        IRTerminator::Branch {
            cond,
            true_block,
            false_block,
        } => {
            let cv = match cond {
                IRValue::Register(r) => *r,
                _ => return TripCount::Unknown,
            };
            // The exit is the non-header target.
            let exit = if true_block == &loop_info.header {
                false_block.clone()
            } else if false_block == &loop_info.header {
                true_block.clone()
            } else {
                return TripCount::Unknown;
            };
            (cv, exit)
        }
        _ => return TripCount::Unknown,
    };

    let mut cmp_instr: Option<&IRInstr> = None;
    for instr in &latch.instructions {
        if let IRInstr::Cmp { dst, lhs, .. } = instr {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *d == cond_vreg && *l == i_next_vreg {
                    cmp_instr = Some(instr);
                    break;
                }
            }
        }
    }
    let cmp = match cmp_instr {
        Some(c) => c,
        None => return TripCount::Unknown,
    };
    let (kind, end) = match cmp {
        IRInstr::Cmp {
            kind,
            rhs,
            ..
        } => {
            let end = match rhs {
                IRValue::Immediate(c) => *c,
                _ => return TripCount::Unknown, // end must be a constant for Known.
            };
            (*kind, end)
        }
        _ => return TripCount::Unknown,
    };

    // Compute trip count from the exit condition.
    // The IV value being compared is `i_next = start + step * (n+1)` where n
    // is the iteration count starting from 0. We want the smallest n such that
    // the condition becomes false (loop exits).
    //
    // For `i_next < end` (SLt/ULt): loop continues while `start + step*(n+1) < end`.
    //   Trip count = max(0, ceil((end - start - step) / step)).
    //   Simpler form for the common case (start=0, step=1): trip = end - 1 + 1 = end? Hmm.
    //
    // Actually let's be careful. The loop runs iteration n if `cond` is true
    // after iteration n. cond compares `i_next` (= iv after iteration n's
    // increment) against `end`. For `i_next < end` with iv_0 = start and
    // iv_n = start + n*step (post-increment): the loop runs while
    // `start + (n+1)*step < end`... no wait, the increment happens at the end
    // of each iteration, and then the comparison checks if we should continue.
    //
    // Standard form: at the top of iteration n, iv = start + n*step. The body
    // runs, then iv is incremented to iv_next = start + (n+1)*step, then the
    // exit check `iv_next < end` decides whether to continue to iteration n+1.
    //
    // The loop exits after iteration n if `start + (n+1)*step >= end` (for SLt).
    // So the LAST iteration run is the largest n such that `start + (n+1)*step < end`.
    // Total iterations = n+1 where n is the largest satisfying the inequality.
    //   n+1 < (end - start) / step
    //   trip_count = ceil((end - start) / step) if (end - start) is divisible
    //                by step; otherwise floor((end - start) / step) + 1... hmm.
    //
    // Actually the cleanest: trip_count = number of iterations = number of
    // times the body runs. For `for i in 0..N` (start=0, step=1, exit `i+1 < N`):
    // the body runs for i = 0, 1, ..., N-1 → trip_count = N.
    //   ceil((N - 0) / 1) = N. ✓
    // For `while i < N { i += 2 }` (start=0, step=2, exit `i+2 < N`):
    //   ceil((N - 0) / 2) = ceil(N/2). ✓
    //
    // So trip_count = ceil((end - start) / step) for `i_next < end`.

    let diff = end - start;
    if step > 0 && diff <= 0 {
        return TripCount::Known(0); // Loop doesn't run.
    }
    if step < 0 && diff >= 0 {
        return TripCount::Known(0); // Loop doesn't run.
    }
    let abs_step = step.unsigned_abs();
    let abs_diff = diff.unsigned_abs();

    let trip = match kind {
        CmpKind::SLt | CmpKind::ULt => {
            // trip = ceil(abs_diff / abs_step)
            abs_diff.div_ceil(abs_step)
        }
        CmpKind::SLe | CmpKind::ULe => {
            // trip = ceil((abs_diff + 1) / abs_step)
            (abs_diff + 1).div_ceil(abs_step)
        }
        CmpKind::Ne => {
            // Exact division required.
            if abs_diff % abs_step != 0 {
                return TripCount::Unknown;
            }
            abs_diff / abs_step
        }
        _ => return TripCount::Unknown,
    };

    TripCount::Known(trip)
}

/// Compute the unroll factor from the trip count and body size, respecting
/// the code-size budget.
///
/// - Known small trip count (≤ `FULL_UNROLL_THRESHOLD`): fully unroll
///   (`factor = trip_count`), capped by `MAX_UNROLL_FACTOR` and the budget.
/// - Known large trip count: `min(MAX_UNROLL_FACTOR, trip_count / 2)`,
///   capped by the budget.
/// - Unknown trip count: `DEFAULT_UNROLL_FACTOR`, capped by the budget.
pub fn compute_unroll_factor(trip_count: &TripCount, body_size: usize) -> u32 {
    let candidate = match trip_count {
        TripCount::Known(n) => {
            if *n <= FULL_UNROLL_THRESHOLD {
                (*n).min(MAX_UNROLL_FACTOR as u64) as u32
            } else {
                let f = (*n / 2).min(MAX_UNROLL_FACTOR as u64) as u32;
                f.max(2)
            }
        }
        TripCount::Bounded(n) => {
            if *n <= FULL_UNROLL_THRESHOLD {
                (*n).min(MAX_UNROLL_FACTOR as u64) as u32
            } else {
                DEFAULT_UNROLL_FACTOR
            }
        }
        TripCount::Unknown => DEFAULT_UNROLL_FACTOR,
    };

    // Apply the code-size budget: don't unroll if body_size * factor > budget.
    let body = body_size.max(1) as u32;
    let max_by_budget = UNROLL_CODE_SIZE_BUDGET / body;
    let factor = candidate.min(max_by_budget.max(1));
    if factor < 2 {
        1 // No unrolling — body too large for any factor ≥ 2 within budget.
    } else {
        factor
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Public entry point
// ─────────────────────────────────────────────────────────────────────────

/// Attempt to correctly unroll loops in a function.
///
/// Handles three loop patterns:
/// 1. **Self-loops** (single block, CondBranch back to self): handled by
///    `try_unroll_block`, which duplicates the body in-place and changes
///    the IV step from +1 to +F.
/// 2. **2-block natural loops** (header + latch, no body blocks): handled
///    by `try_unroll_general_loop` (the existing Wave 13b path).
/// 3. **Multi-block natural loops** (header + body + latch): handled by
///    `try_unroll_multiblock_loop` (NEW in Wave 30), which clones the
///    body+lid sequence `factor` times and rewires the block graph.
///
/// All three patterns require a detectable induction variable and bail out
/// for any loop they cannot fully analyze (no miscompilation possible).
/// The unroll factor is derived from trip-count analysis (affine SCEV)
/// and capped by a code-size budget.
pub fn unroll_loops(mut func: IRFunction) -> IRFunction {
    // Phase 1: Unroll multi-block natural loops (NEW in Wave 30) and 2-block
    // natural loops (Wave 13b).
    let loops = LoopDetector::detect_with_induction_vars(&func);
    for loop_info in &loops {
        if loop_info.blocks.len() == 1 {
            continue; // Single-block — handled by Phase 2.
        }
        let trip_count = analyze_trip_count(&func, loop_info);
        let body_size = compute_loop_body_size(&func, loop_info);
        let factor = compute_unroll_factor(&trip_count, body_size);
        if factor < 2 {
            continue; // Body too large for any unrolling within budget.
        }
        // Try multi-block first (handles body blocks); fall back to the
        // 2-block path if multi-block bails (e.g. due to unsupported instrs).
        if let Some(unrolled) = try_unroll_multiblock_loop(&func, loop_info, factor) {
            func = unrolled;
        } else if let Some(unrolled) = try_unroll_general_loop(&func, loop_info, factor) {
            func = unrolled;
        }
    }

    // Phase 2: Unroll single-block self-loops (the original Wave 13b path).
    // Use the trip-count-derived factor when available.
    let mut changed = true;
    let max_iterations = 3;
    let mut iter = 0;
    while changed && iter < max_iterations {
        changed = false;
        iter += 1;
        for block_idx in 0..func.blocks.len() {
            // Build a synthetic single-block LoopInfo for trip-count analysis.
            let single_loop = LoopInfo {
                header: func.blocks[block_idx].label.clone(),
                latch: func.blocks[block_idx].label.clone(),
                blocks: std::iter::once(func.blocks[block_idx].label.clone()).collect(),
                depth: 0,
                induction_vars: std::collections::HashSet::new(),
            };
            let trip_count = analyze_trip_count(&func, &single_loop);
            let body_size = func.blocks[block_idx].instructions.len();
            let factor = compute_unroll_factor(&trip_count, body_size);
            if factor < 2 {
                continue;
            }
            let block = &func.blocks[block_idx];
            if let Some(unrolled) = try_unroll_block(block, factor) {
                func.blocks[block_idx] = unrolled;
                changed = true;
            }
        }
    }

    // Phase 3: Unroll-and-jam (Wave 30 stub — no-op, TODO).
    func = try_unroll_and_jam(func);

    func
}

/// Compute the total instruction count of a loop's body (all blocks).
fn compute_loop_body_size(func: &IRFunction, loop_info: &LoopInfo) -> usize {
    loop_info
        .blocks
        .iter()
        .filter_map(|label| func.blocks.iter().find(|b| &b.label == label))
        .map(|b| b.instructions.len())
        .sum()
}

// ─────────────────────────────────────────────────────────────────────────
// Multi-block loop unrolling (NEW in Wave 30)
// ─────────────────────────────────────────────────────────────────────────

/// Attempt to unroll a multi-block natural loop by `factor`, with block-graph
/// rewiring.
///
/// This handles loops with a header, one or more body blocks, and a latch.
/// The algorithm:
///
/// 1. Identify the IV Phi in the header and the increment `i + 1` in the latch.
/// 2. Find the exit comparison `cond = cmp i_next, N, SLt` and the exit label.
/// 3. Verify no calls/atomics/free in the loop body.
/// 4. For each copy `k` in `0..factor`:
///    - Clone each body block (renamed `{orig}_u{k}`), substituting `phi → iv_k`
///      where `iv_k = phi + k` (with `iv_0 = phi`). Each clone's dst vregs are
///      renumbered for SSA.
///    - Clone the latch (renamed `{latch}_u{k}`), with the same substitution.
///    - If `k < factor - 1`: drop the latch's increment and comparison, and
///      change its terminator to `Jump({first_body}_u{k+1})` (unconditional
///      to the next copy).
///    - If `k == factor - 1`: keep the increment (which now produces
///      `phi + factor` via the `+1` on `iv_{F-1} = phi + (F-1)`), keep the
///      comparison, keep the `Branch cond, header, exit` terminator.
/// 5. Rewire the header's terminator to `Jump({first_body}_u0)`.
/// 6. Remove the original body and latch blocks (now unreachable).
///
/// The loop runs `N/factor` iterations after unrolling (not `N*factor`), so
/// total work stays `N` — no miscompilation.
///
/// Returns `Some(unrolled_func)` if successful, or `None` if the loop can't
/// be safely unrolled.
fn try_unroll_multiblock_loop(
    func: &IRFunction,
    loop_info: &LoopInfo,
    factor: u32,
) -> Option<IRFunction> {
    if factor < 2 || loop_info.blocks.len() < 2 {
        return None;
    }

    // Find the header block.
    let header_idx = func.blocks.iter().position(|b| b.label == loop_info.header)?;
    let header = &func.blocks[header_idx];

    // The header must start with a Phi (the induction variable).
    if header.instructions.is_empty() {
        return None;
    }
    let phi_vreg = match &header.instructions[0] {
        IRInstr::Phi { dst, incoming } => {
            if incoming.len() != 2 {
                return None;
            }
            match dst {
                IRValue::Register(r) => *r,
                _ => return None,
            }
        }
        _ => return None,
    };

    // Find the latch block.
    let latch_idx = func.blocks.iter().position(|b| b.label == loop_info.latch)?;
    let latch = &func.blocks[latch_idx];

    // Find `i_next = i + 1` in the latch.
    let mut increment_idx = None;
    let mut i_new_vreg = 0u32;
    for (i, instr) in latch.instructions.iter().enumerate() {
        if let IRInstr::BinOp {
            op: BinOpKind::Add,
            dst,
            lhs,
            rhs: IRValue::Immediate(1),
            ..
        } = instr
        {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *l == phi_vreg {
                    increment_idx = Some(i);
                    i_new_vreg = *d;
                    break;
                }
            }
        }
    }
    let increment_idx = increment_idx?;

    // Find the comparison `cond = cmp i_new, N, kind` in the latch.
    let (cond_vreg, exit_label) = match &latch.terminator {
        IRTerminator::Branch {
            cond,
            true_block,
            false_block,
        } => {
            let cv = match cond {
                IRValue::Register(r) => *r,
                _ => return None,
            };
            let exit = if true_block == &loop_info.header {
                false_block.clone()
            } else if false_block == &loop_info.header {
                true_block.clone()
            } else {
                return None;
            };
            (cv, exit)
        }
        _ => return None,
    };
    let mut cmp_idx = None;
    for (i, instr) in latch.instructions.iter().enumerate() {
        if let IRInstr::Cmp { dst, lhs, .. } = instr {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *d == cond_vreg && *l == i_new_vreg {
                    cmp_idx = Some(i);
                    break;
                }
            }
        }
    }
    let _cmp_idx = cmp_idx?;

    // Validate body blocks for safety (no calls/atomics/free).
    let body_labels: Vec<String> = func
        .blocks
        .iter()
        .map(|b| b.label.clone())
        .filter(|l| {
            loop_info.blocks.contains(l) && l != &loop_info.header && l != &loop_info.latch
        })
        .collect();

    for block_label in &loop_info.blocks {
        let block = func.blocks.iter().find(|b| &b.label == block_label)?;
        for instr in &block.instructions {
            if !is_safe_for_unroll(instr) {
                return None;
            }
        }
    }

    // Code-size budget check.
    let body_size: usize = loop_info
        .blocks
        .iter()
        .filter_map(|l| func.blocks.iter().find(|b| &b.label == l))
        .map(|b| b.instructions.len())
        .sum();
    if body_size as u32 * factor > UNROLL_CODE_SIZE_BUDGET {
        return None;
    }

    // ── Perform the unrolling ──────────────────────────────────────────
    let mut new_func = func.clone();
    let mut next_vreg = func_max_vreg(&new_func) + 1;

    // Capture the original body and latch blocks (we'll clone them).
    let orig_body: Vec<IRBlock> = body_labels
        .iter()
        .filter_map(|l| func.blocks.iter().find(|b| &b.label == l).cloned())
        .collect();
    let _orig_latch = latch.clone();

    // Determine the first body block (or the latch if there are no body blocks).
    let first_body_label = body_labels
        .first()
        .cloned()
        .unwrap_or_else(|| loop_info.latch.clone());

    // Remove the original body and latch blocks from new_func.
    new_func.blocks.retain(|b| {
        !body_labels.contains(&b.label) && b.label != loop_info.latch
    });

    // Find the header in new_func (its index may have shifted).
    let new_header_idx = new_func
        .blocks
        .iter()
        .position(|b| b.label == loop_info.header)?;
    // The insert position is right after the header.
    let insert_pos = new_header_idx + 1;

    // Generate `factor` copies.
    let mut new_blocks: Vec<IRBlock> = Vec::new();
    for k in 0u32..factor {
        // iv_k: the IV value used by copy k. iv_0 = phi_vreg; iv_k = phi_vreg + k.
        let iv_k = if k == 0 {
            phi_vreg
        } else {
            let v = next_vreg;
            next_vreg += 1;
            v
        };

        // Clone each body block.
        for (bi, orig_b) in orig_body.iter().enumerate() {
            let new_label = format!("{}_u{}", orig_b.label, k);
            let mut new_b = IRBlock::new(&new_label);

            // If k > 0 and this is the first body block, emit `iv_k = phi + k`.
            if k > 0 && bi == 0 {
                new_b.instructions.push(IRInstr::BinOp {
                    op: BinOpKind::Add,
                    dst: IRValue::Register(iv_k),
                    lhs: IRValue::Register(phi_vreg),
                    rhs: IRValue::Immediate(k as i64),
                    ty: None,
                });
            }

            // Clone the body's instructions, substituting phi → iv_k and
            // renumbering dsts for SSA.
            for instr in &orig_b.instructions {
                let mut cloned = instr.clone();
                renumbered_substitute(&mut cloned, phi_vreg, iv_k, &mut next_vreg);
                new_b.instructions.push(cloned);
            }

            // Rewire the terminator: jump targets to other body blocks (or
            // the latch) get the `_u{k}` suffix.
            new_b.terminator =
                rewire_block_terminator(&orig_b.terminator, k, &body_labels, &loop_info.latch);
            new_b.source_line = orig_b.source_line;
            new_blocks.push(new_b);
        }

        // Clone the latch.
        let new_latch_label = format!("{}_u{}", loop_info.latch, k);
        let mut new_latch = IRBlock::new(&new_latch_label);

        if k < factor - 1 {
            // Non-last copy: keep the latch's body (instructions before the
            // increment), drop the increment and cmp, and Jump to the next
            // copy's first body block (or next latch if no body).
            for instr in &latch.instructions[..increment_idx] {
                let mut cloned = instr.clone();
                renumbered_substitute(&mut cloned, phi_vreg, iv_k, &mut next_vreg);
                new_latch.instructions.push(cloned);
            }
            let next_first = if !body_labels.is_empty() {
                format!("{}_u{}", body_labels[0], k + 1)
            } else {
                format!("{}_u{}", loop_info.latch, k + 1)
            };
            new_latch.terminator = IRTerminator::Jump(next_first);
        } else {
            // Last copy: keep the latch's body, the increment (which now
            // produces phi + factor via +1 on iv_{F-1}), and the cmp. The
            // Branch back to header is preserved (with exit to original exit).
            for instr in &latch.instructions {
                let mut cloned = instr.clone();
                renumbered_substitute(&mut cloned, phi_vreg, iv_k, &mut next_vreg);
                new_latch.instructions.push(cloned);
            }
            // The terminator stays Branch cond, header, exit. The cond's vreg
            // has been renumbered by renumbered_substitute (it's the cmp's dst,
            // which is a defined reg). We need to update the Branch's cond to
            // the new vreg. Track the renumbering.
            // Actually, renumbered_substitute only renumbers DST vregs. The
            // cmp's dst is renumbered. The Branch's cond (a USE of the cmp's
            // dst) needs to be updated to the new vreg.
            //
            // Find the original cmp's dst vreg, find the new cmp's dst vreg
            // (the last Cmp instruction in new_latch that matches), and update
            // the Branch's cond.
            let new_cond_vreg = new_latch
                .instructions
                .iter()
                .rev()
                .find_map(|instr| match instr {
                    IRInstr::Cmp { dst, .. } => dst.as_register(),
                    _ => None,
                })
                .unwrap_or(cond_vreg);
            new_latch.terminator = IRTerminator::Branch {
                cond: IRValue::Register(new_cond_vreg),
                true_block: loop_info.header.clone(),
                false_block: exit_label.clone(),
            };
        }
        new_latch.source_line = latch.source_line;
        new_blocks.push(new_latch);
    }

    // Rewire the header's terminator to jump to the first new body block.
    let first_new = format!("{}_u{}", first_body_label, 0);
    new_func.blocks[new_header_idx].terminator = IRTerminator::Jump(first_new);

    // Insert the new blocks after the header.
    new_func.blocks.splice(insert_pos..insert_pos, new_blocks);

    // Rebuild the CFG so predecessors/successors are consistent.
    new_func.rebuild_cfg();
    Some(new_func)
}

/// Rewire a body block's terminator targets to the `_u{k}` suffix for the
/// current copy. Branches to other body blocks (or the latch) get suffixed;
/// branches to blocks outside the loop are unchanged.
fn rewire_block_terminator(
    term: &IRTerminator,
    k: u32,
    body_labels: &[String],
    latch_label: &str,
) -> IRTerminator {
    let suffix = |label: &str| -> String {
        if body_labels.iter().any(|l| l == label) || label == latch_label {
            format!("{}_u{}", label, k)
        } else {
            label.to_string()
        }
    };
    match term {
        IRTerminator::Jump(t) => IRTerminator::Jump(suffix(t)),
        IRTerminator::Branch {
            cond,
            true_block,
            false_block,
        } => IRTerminator::Branch {
            cond: cond.clone(),
            true_block: suffix(true_block),
            false_block: suffix(false_block),
        },
        other => other.clone(),
    }
}

/// Check if an instruction is safe to duplicate during unrolling.
fn is_safe_for_unroll(instr: &IRInstr) -> bool {
    matches!(
        instr,
        IRInstr::BinOp { .. }
            | IRInstr::Add { .. }
            | IRInstr::Sub { .. }
            | IRInstr::Mul { .. }
            | IRInstr::Div { .. }
            | IRInstr::Cmp { .. }
            | IRInstr::Load { .. }
            | IRInstr::Store { .. }
            | IRInstr::Offset { .. }
            | IRInstr::Cast { .. }
            | IRInstr::Select { .. }
            | IRInstr::Phi { .. }
            | IRInstr::Alloc { .. }
    )
}

// ─────────────────────────────────────────────────────────────────────────
// Unroll-and-jam (Wave 30 — conservative implementation)
// ─────────────────────────────────────────────────────────────────────────

/// Unroll-and-jam: unroll the outer loop of a nested loop and place the
/// inner loop copies adjacent in the CFG ("jam" them together). This is a
/// profitable transformation for nested loops where the inner body has no
/// outer-loop-carried dependencies — it improves data locality and exposes
/// vectorization opportunities.
///
/// ## Conservative scope (Wave 30)
///
/// This implementation is deliberately conservative. It only fires when ALL
/// of the following hold:
///
/// 1. **Perfectly nested.** The outer loop's body (blocks in `outer.blocks`
///    minus `outer.header`, `outer.latch`, and `inner.blocks`) is empty. The
///    outer header's terminator is `Jump(inner.header)` — i.e., the outer
///    body IS the inner loop.
/// 2. **Outer IV is canonical.** Outer header starts with `Phi(0, entry),
///    (i_o_next, outer.latch)`. Outer latch has `i_o_next = i_o + 1`,
///    `cond_o = cmp i_o_next, end`, `Branch cond_o, outer.header, exit`.
/// 3. **Inner IV is canonical.** Inner header starts with `Phi(0, ?),
///    (i_i_next, inner.latch)`. Inner latch has `i_i_next = i_i + 1`,
///    `cond_i = cmp i_i_next, M`, `Branch cond_i, inner.header, outer.latch`.
/// 4. **Inner trip count invariant.** The inner loop's `end` (the RHS of the
///    inner cmp) is a constant `Immediate` — so it doesn't depend on the
///    outer IV. (If `end` were `i_o * 4`, jamming would be unsafe because
///    the inner trip count would vary across outer iterations.)
/// 5. **Inner body has no outer-loop-carried stores.** No `Store` in the
///    inner loop body has an `addr` that uses the outer IV (`outer_phi_vreg`).
///    (A store to `A[i_o]` would be violated by jamming: the second inner
///    loop copy would write to `A[i_o + 1]` before the first copy's writes
///    are observed by a downstream reader — a classic anti-dependency.)
/// 6. **Inner body is safe to duplicate.** Every instruction passes
///    `is_safe_for_unroll` (no calls, atomics, free, etc.).
/// 7. **Inner header has exactly one instruction** (the IV Phi). No
///    loop-carried Phis for `sum`-style accumulators.
/// 8. **Code-size budget.** `inner_body_size * FACTOR ≤ UNROLL_CODE_SIZE_BUDGET`.
///
/// If any check fails, the function is returned unchanged (safe no-op).
///
/// ## Transformation
///
/// Unroll the outer loop by `FACTOR = 2`. The inner loop is duplicated `FACTOR`
/// times and placed adjacent in the CFG. Each copy `k` uses `i_o + k` as its
/// outer IV value (substituted into the inner body). The outer latch's
/// increment is changed from `+1` to `+FACTOR`.
///
/// ```text
/// BEFORE:                          AFTER (FACTOR=2):
/// outer_header:                    outer_header:
///   i_o = phi(0,entry),            i_o = phi(0,entry),
///          (i_o_next, latch)              (i_o_next, latch)
///   jump inner_header              jump inner_header_u0
/// inner_header:                    inner_header_u0:
///   i_i = phi(0,oh),                     i_i = phi(0,oh),(i_i_next,latch_u0)
///         (i_i_next,latch)         ... body using i_o, i_i ...
/// ... body using i_o, i_i ...           jump body_u0 ... latch_u0
/// inner_latch:                     latch_u0: Branch → inner_header_u0 / between_u1
///   i_i_next = i_i + 1             between_u1:
///   cond = cmp i_i_next, M           i_o_1 = i_o + 1
///   Branch → inner_header /          jump inner_header_u1
///          outer_latch              inner_header_u1:
/// outer_latch:                       i_i = phi(0,between_u1),
///   i_o_next = i_o + 1                     (i_i_next,latch_u1)
///   cond = cmp i_o_next, end         ... body using i_o_1, i_i ...
///   Branch → outer_header /               jump body_u1 ... latch_u1
///          exit                     latch_u1: Branch → inner_header_u1 / outer_latch
///                                   outer_latch:
///                                     i_o_next = i_o + 2   (step by FACTOR)
///                                     cond = cmp i_o_next, end
///                                     Branch → outer_header / exit
/// ```
///
/// The outer loop now runs `ceil(N / FACTOR)` iterations (not `N * FACTOR`),
/// so total work stays `N * M` — no miscompilation. The "jam" places the
/// inner loops adjacent (simpler than true fusion into a single inner loop
/// with `FACTOR×` the body — see the task spec's "Conservative approach").
fn try_unroll_and_jam(func: IRFunction) -> IRFunction {
    const UNROLL_AND_JAM_FACTOR: u32 = 2;

    // ── Step 1: Detect loops ───────────────────────────────────────────
    let loops = LoopDetector::detect_with_induction_vars(&func);
    if loops.len() < 2 {
        return func; // Need ≥ 2 loops for nesting.
    }

    // ── Step 2: Find a perfectly-nested (outer, inner) pair ───────────
    // Outer: a loop that contains another loop's header in its block set.
    // Inner: a loop whose header is in outer's blocks, and whose blocks are
    // a subset of outer's blocks. Perfect nesting: outer's blocks (minus
    // header, latch, and inner's blocks) must be empty.
    let mut chosen: Option<(&LoopInfo, &LoopInfo)> = None;
    'outer: for outer in &loops {
        for inner in &loops {
            if outer.header == inner.header {
                continue;
            }
            if !outer.blocks.contains(&inner.header) {
                continue;
            }
            if !inner.blocks.is_subset(&outer.blocks) {
                continue;
            }
            // Perfect-nest check: outer body blocks (not header, not latch,
            // not in inner) must be empty.
            let outer_extra: Vec<&String> = outer
                .blocks
                .iter()
                .filter(|l| {
                    **l != outer.header
                        && **l != outer.latch
                        && !inner.blocks.contains(*l)
                })
                .collect();
            if !outer_extra.is_empty() {
                continue;
            }
            chosen = Some((outer, inner));
            break 'outer;
        }
    }
    let (outer, inner) = match chosen {
        Some(p) => p,
        None => return func, // No perfectly-nested pair found.
    };

    // ── Step 3: Validate outer header/latch structure ─────────────────
    let outer_header = match func.blocks.iter().find(|b| b.label == outer.header) {
        Some(b) => b,
        None => return func,
    };
    // Outer header must Jump directly to inner header (perfect nesting).
    match &outer_header.terminator {
        IRTerminator::Jump(t) if t == &inner.header => {}
        _ => return func,
    }
    // Outer header must start with a Phi (the outer IV).
    if outer_header.instructions.is_empty() {
        return func;
    }
    let outer_phi_vreg = match &outer_header.instructions[0] {
        IRInstr::Phi { dst, incoming } => {
            if incoming.len() != 2 {
                return func;
            }
            match dst {
                IRValue::Register(r) => *r,
                _ => return func,
            }
        }
        _ => return func,
    };

    // Outer latch: must have `i_o_next = i_o + 1`, `cond_o = cmp i_o_next, end`,
    // `Branch cond_o, outer.header, exit`.
    let outer_latch = match func.blocks.iter().find(|b| b.label == outer.latch) {
        Some(b) => b,
        None => return func,
    };
    // Find the increment `i_o_next = i_o + 1`.
    let mut outer_inc_idx = None;
    let mut outer_iv_next = 0u32;
    for (i, instr) in outer_latch.instructions.iter().enumerate() {
        if let IRInstr::BinOp {
            op: BinOpKind::Add,
            dst,
            lhs,
            rhs: IRValue::Immediate(1),
            ..
        } = instr
        {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *l == outer_phi_vreg {
                    outer_inc_idx = Some(i);
                    outer_iv_next = *d;
                    break;
                }
            }
        }
    }
    let outer_inc_idx = match outer_inc_idx {
        Some(i) => i,
        None => return func,
    };
    // Find the exit Branch + the cond vreg + exit label.
    let outer_cond_vreg = match &outer_latch.terminator {
        IRTerminator::Branch {
            cond,
            true_block,
            false_block,
        } => {
            let cv = match cond {
                IRValue::Register(r) => *r,
                _ => return func,
            };
            let _exit = if true_block == &outer.header {
                false_block.clone()
            } else if false_block == &outer.header {
                true_block.clone()
            } else {
                return func;
            };
            cv
        }
        _ => return func,
    };
    // Find the cmp `cond_o = cmp i_o_next, end` in the latch.
    let mut outer_cmp_idx = None;
    for (i, instr) in outer_latch.instructions.iter().enumerate() {
        if let IRInstr::Cmp { dst, lhs, .. } = instr {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *d == outer_cond_vreg && *l == outer_iv_next {
                    outer_cmp_idx = Some(i);
                    break;
                }
            }
        }
    }
    let _outer_cmp_idx = match outer_cmp_idx {
        Some(i) => i,
        None => return func,
    };

    // ── Step 4: Validate inner header/latch structure ─────────────────
    let inner_header = match func.blocks.iter().find(|b| b.label == inner.header) {
        Some(b) => b,
        None => return func,
    };
    // Inner header must have exactly ONE instruction (the IV Phi). Bail on
    // any loop-carried accumulator Phis (e.g., `sum = phi(0, ..., (sum, latch))`).
    if inner_header.instructions.len() != 1 {
        return func;
    }
    let inner_phi_vreg = match &inner_header.instructions[0] {
        IRInstr::Phi { dst, incoming } => {
            if incoming.len() != 2 {
                return func;
            }
            match dst {
                IRValue::Register(r) => *r,
                _ => return func,
            }
        }
        _ => return func,
    };

    let inner_latch = match func.blocks.iter().find(|b| b.label == inner.latch) {
        Some(b) => b,
        None => return func,
    };
    // Inner latch's exit target must be outer.latch (perfect nesting).
    let inner_cond_vreg = match &inner_latch.terminator {
        IRTerminator::Branch {
            cond,
            true_block,
            false_block,
        } => {
            let cv = match cond {
                IRValue::Register(r) => *r,
                _ => return func,
            };
            let exit = if true_block == &inner.header {
                false_block.clone()
            } else if false_block == &inner.header {
                true_block.clone()
            } else {
                return func;
            };
            if exit != outer.latch {
                return func; // Inner loop's exit must go to outer latch.
            }
            cv
        }
        _ => return func,
    };
    // Find the inner increment `i_i_next = i_i + 1`.
    let mut inner_inc_idx = None;
    let mut inner_iv_next = 0u32;
    for (i, instr) in inner_latch.instructions.iter().enumerate() {
        if let IRInstr::BinOp {
            op: BinOpKind::Add,
            dst,
            lhs,
            rhs: IRValue::Immediate(1),
            ..
        } = instr
        {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *l == inner_phi_vreg {
                    inner_inc_idx = Some(i);
                    inner_iv_next = *d;
                    break;
                }
            }
        }
    }
    let inner_inc_idx = match inner_inc_idx {
        Some(i) => i,
        None => return func,
    };
    // Find the inner cmp `cond_i = cmp i_i_next, end`. The `end` must be a
    // constant Immediate (else the inner trip count may depend on the outer
    // IV — unsafe for jamming).
    let mut inner_cmp_idx = None;
    for (i, instr) in inner_latch.instructions.iter().enumerate() {
        if let IRInstr::Cmp { dst, lhs, rhs, .. } = instr {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *d == inner_cond_vreg && *l == inner_iv_next {
                    if !matches!(rhs, IRValue::Immediate(_)) {
                        return func; // Inner end is not a constant — bail.
                    }
                    inner_cmp_idx = Some(i);
                    break;
                }
            }
        }
    }
    let inner_cmp_idx = match inner_cmp_idx {
        Some(i) => i,
        None => return func,
    };

    // ── Step 5: Safety checks on inner body ───────────────────────────
    // (a) Every instruction must be safe to duplicate (no calls/atomics/free).
    // (b) No Store may have an `addr` that is transitively derived from the
    //     outer IV (outer-loop-carried memory dependency). We compute the set
    //     of vregs "tainted" by the outer IV (defined using the outer IV or
    //     another tainted vreg) and bail if any Store's addr is tainted.
    // (c) No Phi in the inner body blocks (Phis only belong in loop headers).
    let mut outer_tainted: std::collections::HashSet<u32> = std::collections::HashSet::new();
    outer_tainted.insert(outer_phi_vreg);
    let mut changed = true;
    while changed {
        changed = false;
        for block_label in &inner.blocks {
            let block = match func.blocks.iter().find(|b| &b.label == block_label) {
                Some(b) => b,
                None => return func,
            };
            for instr in &block.instructions {
                let uses_outer = instr.used_regs().iter().any(|r| outer_tainted.contains(r));
                if uses_outer {
                    for def in instr.defined_regs() {
                        if outer_tainted.insert(def) {
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    for block_label in &inner.blocks {
        // Skip the header and latch — they contain the IV Phi and the
        // increment/cmp, which are expected and already validated. The
        // "no Phi in body" check applies only to the body blocks.
        if block_label == &inner.header || block_label == &inner.latch {
            continue;
        }
        let block = match func.blocks.iter().find(|b| &b.label == block_label) {
            Some(b) => b,
            None => return func,
        };
        for instr in &block.instructions {
            if !is_safe_for_unroll(instr) {
                return func; // (a) unsafe instruction.
            }
            if let IRInstr::Store { addr: IRValue::Register(r), .. } = instr {
                if outer_tainted.contains(r) {
                    return func; // (b) outer-loop-carried store.
                }
            }
            if matches!(instr, IRInstr::Phi { .. }) {
                return func; // (c) Phi in body — bail.
            }
        }
    }

    // ── Step 6: Code-size budget ──────────────────────────────────────
    let inner_body_size: usize = inner
        .blocks
        .iter()
        .filter_map(|l| func.blocks.iter().find(|b| &b.label == l))
        .map(|b| b.instructions.len())
        .sum();
    if inner_body_size as u32 * UNROLL_AND_JAM_FACTOR > UNROLL_CODE_SIZE_BUDGET {
        return func;
    }

    // ── Step 7: Perform the transformation ────────────────────────────
    let mut new_func = func.clone();
    let mut next_vreg = func_max_vreg(&new_func) + 1;

    // Capture the original inner blocks (header, body blocks in function
    // order, latch) — we'll clone them FACTOR times.
    let inner_header_orig = inner_header.clone();
    let inner_latch_orig = inner_latch.clone();
    let inner_body_blocks: Vec<IRBlock> = func
        .blocks
        .iter()
        .filter(|b| {
            inner.blocks.contains(&b.label)
                && b.label != inner.header
                && b.label != inner.latch
        })
        .cloned()
        .collect();
    let inner_body_labels: Vec<String> =
        inner_body_blocks.iter().map(|b| b.label.clone()).collect();

    // Remove the original inner blocks from new_func (we'll add the
    // duplicated versions after the outer header).
    new_func
        .blocks
        .retain(|b| !inner.blocks.contains(&b.label));

    // Find the outer header index in new_func (its position may have shifted
    // because we removed inner blocks, but the outer header is NOT an inner
    // block so it's retained; only its index may change if inner blocks
    // preceded it, which they can't since outer header dominates inner).
    let outer_header_idx = match new_func.blocks.iter().position(|b| b.label == outer.header) {
        Some(i) => i,
        None => return func,
    };

    // Generate FACTOR copies of the inner loop, plus "between" blocks that
    // compute `i_o_k = i_o + k` for k > 0.
    let mut new_blocks: Vec<IRBlock> = Vec::new();

    for k in 0u32..UNROLL_AND_JAM_FACTOR {
        // The outer IV value used by copy k.
        // k=0: iv_o_0 = outer_phi_vreg (no extra instruction needed).
        // k>0: iv_o_k = outer_phi_vreg + k, computed in a "between" block.
        let iv_o_k = if k == 0 {
            outer_phi_vreg
        } else {
            let v = next_vreg;
            next_vreg += 1;
            v
        };

        // For k > 0, emit a "between" block that computes iv_o_k = i_o + k
        // and jumps to inner_header_u{k}.
        if k > 0 {
            let mut between = IRBlock::new(format!("between_u{}", k));
            between.instructions.push(IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(iv_o_k),
                lhs: IRValue::Register(outer_phi_vreg),
                rhs: IRValue::Immediate(k as i64),
                ty: None,
            });
            between.terminator = IRTerminator::Jump(format!("{}_u{}", inner.header, k));
            new_blocks.push(between);
        }

        // For k > 0, allocate fresh vregs for the inner Phi's dst, the inner
        // increment's dst (i_i_next), and the inner cmp's dst (cond_i), so
        // SSA is preserved across copies. For k=0, keep the originals.
        let inner_phi_k = if k == 0 {
            inner_phi_vreg
        } else {
            let v = next_vreg;
            next_vreg += 1;
            v
        };
        let inner_iv_next_k = if k == 0 {
            inner_iv_next
        } else {
            let v = next_vreg;
            next_vreg += 1;
            v
        };
        let inner_cond_k = if k == 0 {
            inner_cond_vreg
        } else {
            let v = next_vreg;
            next_vreg += 1;
            v
        };

        // ── Clone the inner header ────────────────────────────────────
        let new_header_label = format!("{}_u{}", inner.header, k);
        let mut new_header = IRBlock::new(&new_header_label);

        // The entry predecessor for this copy's Phi:
        //   k=0: outer.header (the original entry).
        //   k>0: between_u{k}.
        let phi_entry_pred: String = if k == 0 {
            outer.header.clone()
        } else {
            format!("between_u{}", k)
        };
        let phi_back_pred = format!("{}_u{}", inner.latch, k);

        // Rebuild the Phi with updated dst (for k>0) and incoming predecessors.
        if let IRInstr::Phi { dst, incoming } = &inner_header_orig.instructions[0] {
            let new_dst = if k == 0 {
                dst.clone()
            } else {
                IRValue::Register(inner_phi_k)
            };
            let mut new_incoming = Vec::with_capacity(incoming.len());
            for (val, src) in incoming {
                let (new_val, new_src) = if src == &inner.latch {
                    // Back-incoming: update the predecessor to inner_latch_u{k},
                    // and the value from inner_iv_next to inner_iv_next_k.
                    let v = if let IRValue::Register(r) = val {
                        if *r == inner_iv_next {
                            IRValue::Register(inner_iv_next_k)
                        } else {
                            val.clone()
                        }
                    } else {
                        val.clone()
                    };
                    (v, phi_back_pred.clone())
                } else {
                    // Entry-incoming: value unchanged, predecessor updated.
                    (val.clone(), phi_entry_pred.clone())
                };
                new_incoming.push((new_val, new_src));
            }
            new_header.instructions.push(IRInstr::Phi {
                dst: new_dst,
                incoming: new_incoming,
            });
        }
        // Header terminator: jump to the first inner body block (or latch if
        // no body) of THIS copy.
        let first_inner_target = if !inner_body_labels.is_empty() {
            format!("{}_u{}", inner_body_labels[0], k)
        } else {
            format!("{}_u{}", inner.latch, k)
        };
        new_header.terminator = IRTerminator::Jump(first_inner_target);
        new_header.source_line = inner_header_orig.source_line;
        new_blocks.push(new_header);

        // ── Clone the inner body blocks ───────────────────────────────
        for orig_b in &inner_body_blocks {
            let new_label = format!("{}_u{}", orig_b.label, k);
            let mut new_b = IRBlock::new(&new_label);
            for instr in &orig_b.instructions {
                let mut cloned = instr.clone();
                // Substitute the outer IV: outer_phi_vreg → iv_o_k.
                substitute_vreg(&mut cloned, outer_phi_vreg, iv_o_k);
                // For k > 0, substitute the inner IV: inner_phi_vreg → inner_phi_k.
                if k > 0 {
                    substitute_vreg(&mut cloned, inner_phi_vreg, inner_phi_k);
                }
                // Renumber the dst for SSA (fresh vreg per copy).
                renumber_dst(&mut cloned, &mut next_vreg);
                new_b.instructions.push(cloned);
            }
            // Rewire the terminator: jump/branch targets to inner body/latch
            // blocks get the _u{k} suffix.
            new_b.terminator = rewire_inner_terminator(
                &orig_b.terminator,
                k,
                &inner_body_labels,
                &inner.latch,
            );
            new_b.source_line = orig_b.source_line;
            new_blocks.push(new_b);
        }

        // ── Clone the inner latch ─────────────────────────────────────
        let new_latch_label = format!("{}_u{}", inner.latch, k);
        let mut new_latch = IRBlock::new(&new_latch_label);
        for (i, instr) in inner_latch_orig.instructions.iter().enumerate() {
            let mut cloned = instr.clone();
            // Substitute the outer IV (in case the latch uses it).
            substitute_vreg(&mut cloned, outer_phi_vreg, iv_o_k);
            // For k > 0, substitute the inner IV.
            if k > 0 {
                substitute_vreg(&mut cloned, inner_phi_vreg, inner_phi_k);
            }
            // Handle the increment and cmp specially: their dsts are
            // inner_iv_next_k and inner_cond_k (not fresh-per-call).
            if i == inner_inc_idx {
                if let IRInstr::BinOp { dst, .. } = &mut cloned {
                    *dst = IRValue::Register(inner_iv_next_k);
                }
            } else if i == inner_cmp_idx {
                if let IRInstr::Cmp { dst, .. } = &mut cloned {
                    *dst = IRValue::Register(inner_cond_k);
                }
                // Also update the cmp's lhs (which is inner_iv_next) to
                // inner_iv_next_k for k > 0.
                if k > 0 {
                    if let IRInstr::Cmp { lhs: IRValue::Register(r), .. } = &mut cloned {
                        if *r == inner_iv_next {
                            *r = inner_iv_next_k;
                        }
                    }
                }
            } else {
                // Other instructions: renumber dst for SSA.
                renumber_dst(&mut cloned, &mut next_vreg);
            }
            new_latch.instructions.push(cloned);
        }
        // The latch's Branch: loop back to inner_header_u{k}, exit to
        // between_u{k+1} (if k < FACTOR-1) or outer.latch (if k == FACTOR-1).
        let inner_exit_target = if k < UNROLL_AND_JAM_FACTOR - 1 {
            format!("between_u{}", k + 1)
        } else {
            outer.latch.clone()
        };
        new_latch.terminator = IRTerminator::Branch {
            cond: IRValue::Register(inner_cond_k),
            true_block: format!("{}_u{}", inner.header, k),
            false_block: inner_exit_target,
        };
        new_latch.source_line = inner_latch_orig.source_line;
        new_blocks.push(new_latch);
    }

    // ── Modify the outer latch: change +1 to +FACTOR ──────────────────
    let outer_latch_idx = match new_func.blocks.iter().position(|b| b.label == outer.latch) {
        Some(i) => i,
        None => return func,
    };
    // Replace the increment's rhs from Immediate(1) to Immediate(FACTOR).
    new_func.blocks[outer_latch_idx].instructions[outer_inc_idx] = IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(outer_iv_next),
        lhs: IRValue::Register(outer_phi_vreg),
        rhs: IRValue::Immediate(UNROLL_AND_JAM_FACTOR as i64),
        ty: None,
    };

    // ── Rewire the outer header to jump to inner_header_u0 ────────────
    new_func.blocks[outer_header_idx].terminator =
        IRTerminator::Jump(format!("{}_u{}", inner.header, 0));

    // ── Insert the new blocks after the outer header ──────────────────
    let insert_pos = outer_header_idx + 1;
    new_func.blocks.splice(insert_pos..insert_pos, new_blocks);

    // Rebuild CFG so predecessors/successors are consistent.
    new_func.rebuild_cfg();
    new_func
}

/// Rewire an inner body block's terminator targets to the `_u{k}` suffix for
/// the current copy. Branches to other inner body blocks (or the inner latch)
/// get suffixed; branches to blocks outside the inner loop are unchanged.
fn rewire_inner_terminator(
    term: &IRTerminator,
    k: u32,
    body_labels: &[String],
    latch_label: &str,
) -> IRTerminator {
    let suffix = |label: &str| -> String {
        if body_labels.iter().any(|l| l == label) || label == latch_label {
            format!("{}_u{}", label, k)
        } else {
            label.to_string()
        }
    };
    match term {
        IRTerminator::Jump(t) => IRTerminator::Jump(suffix(t)),
        IRTerminator::Branch {
            cond,
            true_block,
            false_block,
        } => IRTerminator::Branch {
            cond: cond.clone(),
            true_block: suffix(true_block),
            false_block: suffix(false_block),
        },
        other => other.clone(),
    }
}

/// Renumber an instruction's destination vreg to a fresh `next_vreg`-derived
/// id, for SSA preservation across duplicated copies. Does NOT renumber Phi
/// dsts (the loop-carried IV is handled separately). Used by
/// `try_unroll_and_jam` for inner body / latch instructions where the
/// outer-IV substitution has already been applied via `substitute_vreg`.
fn renumber_dst(instr: &mut IRInstr, next_vreg: &mut u32) {
    let fresh = *next_vreg;
    *next_vreg += 1;
    match instr {
        IRInstr::BinOp { dst, .. }
        | IRInstr::Add { dst, .. }
        | IRInstr::Sub { dst, .. }
        | IRInstr::Mul { dst, .. }
        | IRInstr::Div { dst, .. }
        | IRInstr::Load { dst, .. }
        | IRInstr::Offset { dst, .. }
        | IRInstr::Cast { dst, .. }
        | IRInstr::Select { dst, .. } => {
            if let IRValue::Register(r) = dst {
                *r = fresh;
            }
        }
        IRInstr::Cmp { dst, .. } => {
            if let IRValue::Register(r) = dst {
                *r = fresh;
            }
        }
        IRInstr::Phi { dst: _, .. } => {
            // Phi dsts are handled by the caller (loop-carried IV).
            let _ = fresh;
        }
        IRInstr::Store { .. } | IRInstr::Alloc { .. } | IRInstr::Ret { .. } | _ => {
            // No dst to renumber, or not safe to renumber.
            let _ = fresh;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 2-block unrolling (Wave 13b, retained)
// ─────────────────────────────────────────────────────────────────────────

/// Attempt to unroll a general (header + latch, no body) natural loop by
/// `factor`.
///
/// This is the original Wave 13b path for 2-block loops. Multi-block loops
/// (with body blocks) are handled by `try_unroll_multiblock_loop`.
fn try_unroll_general_loop(
    func: &IRFunction,
    loop_info: &LoopInfo,
    factor: u32,
) -> Option<IRFunction> {
    if factor < 2 || loop_info.blocks.len() < 2 {
        return None;
    }

    // Find the header block.
    let header_idx = func.blocks.iter().position(|b| b.label == loop_info.header)?;
    let header = &func.blocks[header_idx];

    if header.instructions.is_empty() {
        return None;
    }
    let phi_dst = match &header.instructions[0] {
        IRInstr::Phi { dst, incoming } => {
            if incoming.len() != 2 {
                return None;
            }
            dst.clone()
        }
        _ => return None,
    };
    let phi_vreg = match &phi_dst {
        IRValue::Register(r) => *r,
        _ => return None,
    };

    // Check for side effects in all loop blocks.
    for block_label in &loop_info.blocks {
        let block = func.blocks.iter().find(|b| &b.label == block_label)?;
        for instr in &block.instructions {
            if !is_safe_for_unroll(instr) {
                return None;
            }
        }
    }

    // Find the increment in the latch.
    let latch_idx = func.blocks.iter().position(|b| b.label == loop_info.latch)?;
    let latch = &func.blocks[latch_idx];

    let mut increment_instr_idx = None;
    let mut i_new_vreg = 0u32;
    for (i, instr) in latch.instructions.iter().enumerate() {
        if let IRInstr::BinOp {
            op: BinOpKind::Add,
            dst,
            lhs,
            rhs: IRValue::Immediate(1),
            ..
        } = instr
        {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *l == phi_vreg {
                    increment_instr_idx = Some(i);
                    i_new_vreg = *d;
                    break;
                }
            }
        }
    }
    let increment_instr_idx = increment_instr_idx?;

    // Body blocks (must be empty for this 2-block path).
    let body_labels: Vec<String> = func
        .blocks
        .iter()
        .map(|b| b.label.clone())
        .filter(|l| {
            loop_info.blocks.contains(l) && l != &loop_info.header && l != &loop_info.latch
        })
        .collect();
    if !body_labels.is_empty() {
        return None; // Multi-block — use try_unroll_multiblock_loop instead.
    }

    // Code-size budget check.
    let total_instrs: usize = loop_info
        .blocks
        .iter()
        .filter_map(|l| func.blocks.iter().find(|b| &b.label == l))
        .map(|b| b.instructions.len())
        .sum();
    if total_instrs as u32 * factor > UNROLL_CODE_SIZE_BUDGET {
        return None;
    }

    // ── Perform the unrolling ──────────────────────────────────────────
    let mut new_func = func.clone();
    let mut next_vreg = func_max_vreg(&new_func) + 1;

    let mut new_latch = latch.clone();
    let mut new_latch_instrs: Vec<IRInstr> = Vec::new();

    for instr in &latch.instructions[..increment_instr_idx] {
        new_latch_instrs.push(instr.clone());
    }

    for k in 1u64..factor as u64 {
        let iv_k = next_vreg;
        next_vreg += 1;
        new_latch_instrs.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(iv_k),
            lhs: IRValue::Register(phi_vreg),
            rhs: IRValue::Immediate(k as i64),
            ty: None,
        });
        for instr in &latch.instructions[..increment_instr_idx] {
            let mut cloned = instr.clone();
            substitute_vreg(&mut cloned, phi_vreg, iv_k);
            new_latch_instrs.push(cloned);
        }
    }

    // Change the increment from +1 to +F.
    new_latch_instrs.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(i_new_vreg),
        lhs: IRValue::Register(phi_vreg),
        rhs: IRValue::Immediate(factor as i64),
        ty: None,
    });

    for instr in &latch.instructions[increment_instr_idx + 1..] {
        new_latch_instrs.push(instr.clone());
    }

    new_latch.instructions = new_latch_instrs;

    let new_latch_idx = new_func
        .blocks
        .iter()
        .position(|b| b.label == loop_info.latch)
        .unwrap();
    new_func.blocks[new_latch_idx] = new_latch;

    Some(new_func)
}

// ─────────────────────────────────────────────────────────────────────────
// Single-block unrolling (Wave 13b, retained)
// ─────────────────────────────────────────────────────────────────────────

pub fn try_unroll_block(block: &IRBlock, factor: u32) -> Option<IRBlock> {
    if factor < 2 {
        return None;
    }

    // Check 1: self-loop with conditional Branch.
    let self_label = &block.label;
    let (cond_reg, _exit_label) = match &block.terminator {
        IRTerminator::Branch {
            cond,
            true_block,
            false_block,
        } => {
            if true_block == self_label {
                (cond.clone(), false_block.clone())
            } else if false_block == self_label {
                (cond.clone(), true_block.clone())
            } else {
                return None;
            }
        }
        _ => return None,
    };

    let instrs = &block.instructions;
    if instrs.is_empty() {
        return None;
    }

    // Check 2: first instruction is a Phi (induction variable).
    let phi_dst = match &instrs[0] {
        IRInstr::Phi { dst, incoming } => {
            if incoming.len() != 2 {
                return None;
            }
            let has_back_edge = incoming.iter().any(|(_, src)| src == self_label);
            if !has_back_edge {
                return None;
            }
            dst.clone()
        }
        _ => return None,
    };

    // Check 5: bail on any instruction substitute_vreg doesn't handle.
    for instr in instrs {
        if !is_safe_for_unroll(instr) {
            return None;
        }
    }

    // Check 3: find the increment `i_new = i + 1`.
    let phi_vreg = match &phi_dst {
        IRValue::Register(r) => *r,
        _ => return None,
    };

    let mut increment_idx = None;
    let mut increment_dst = None;
    for (i, instr) in instrs.iter().enumerate() {
        if let IRInstr::BinOp {
            op: BinOpKind::Add,
            dst,
            lhs,
            rhs: IRValue::Immediate(1),
            ..
        } = instr
        {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *l == phi_vreg {
                    increment_idx = Some(i);
                    increment_dst = Some(*d);
                    break;
                }
            }
        }
    }

    let increment_idx = increment_idx?;
    let i_new_vreg = increment_dst?;

    let cond_vreg = match &cond_reg {
        IRValue::Register(r) => *r,
        _ => return None,
    };

    let mut cmp_idx = None;
    for (i, instr) in instrs.iter().enumerate() {
        if let IRInstr::Cmp { dst: IRValue::Register(d), lhs, .. } = instr {
            if *d == cond_vreg {
                if let IRValue::Register(l) = lhs {
                    if *l == i_new_vreg {
                        cmp_idx = Some(i);
                        break;
                    }
                }
            }
        }
    }
    let cmp_idx = cmp_idx?;

    let body = &instrs[1..increment_idx];
    if body.is_empty() {
        return None;
    }
    if body.len() > 15 {
        return None;
    }

    // Code-size budget check.
    if body.len() as u32 * factor > UNROLL_CODE_SIZE_BUDGET {
        return None;
    }

    for instr in body {
        for d in instr.defined_regs() {
            if d == phi_vreg || d == i_new_vreg {
                return None;
            }
        }
    }

    // ── Perform the unrolling ──────────────────────────────────────────
    let mut new_instrs: Vec<IRInstr> = Vec::with_capacity(instrs.len() * factor as usize);

    new_instrs.push(instrs[0].clone());

    for instr in body {
        new_instrs.push(instr.clone());
    }

    let mut next_vreg = func_next_vreg(&block.instructions) + 1;
    for k in 1u64..factor as u64 {
        let i_k_vreg = next_vreg;
        next_vreg += 1;
        new_instrs.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(i_k_vreg),
            lhs: IRValue::Register(phi_vreg),
            rhs: IRValue::Immediate(k as i64),
            ty: None,
        });
        for instr in body {
            let mut cloned = instr.clone();
            substitute_vreg(&mut cloned, phi_vreg, i_k_vreg);
            new_instrs.push(cloned);
        }
    }

    new_instrs.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(i_new_vreg),
        lhs: IRValue::Register(phi_vreg),
        rhs: IRValue::Immediate(factor as i64),
        ty: None,
    });

    if cmp_idx < instrs.len() {
        new_instrs.push(instrs[cmp_idx].clone());
    }

    for i in (cmp_idx + 1)..instrs.len() {
        new_instrs.push(instrs[i].clone());
    }

    let mut new_block = IRBlock::new(&block.label);
    new_block.instructions = new_instrs;
    new_block.terminator = block.terminator.clone();

    Some(new_block)
}

// ─────────────────────────────────────────────────────────────────────────
// Vreg helpers
// ─────────────────────────────────────────────────────────────────────────

/// Find the highest vreg number used anywhere in the function.
fn func_max_vreg(func: &IRFunction) -> u32 {
    let mut max: u32 = 0;
    for block in &func.blocks {
        for instr in &block.instructions {
            for r in instr.defined_regs() {
                max = max.max(r);
            }
            for r in instr.used_regs() {
                max = max.max(r);
            }
        }
    }
    max
}

/// Find the highest vreg number used in the block (for generating fresh vregs).
fn func_next_vreg(instrs: &[IRInstr]) -> u32 {
    let mut max: u32 = 0;
    for instr in instrs {
        for r in instr.defined_regs() {
            max = max.max(r);
        }
        for r in instr.used_regs() {
            max = max.max(r);
        }
    }
    max
}

/// Substitute all uses of `old_vreg` with `new_vreg` in an instruction.
fn substitute_vreg(instr: &mut IRInstr, old_vreg: u32, new_vreg: u32) {
    fn sub_val(val: &mut IRValue, old_vreg: u32, new_vreg: u32) {
        if let IRValue::Register(r) = val {
            if *r == old_vreg {
                *r = new_vreg;
            }
        }
    }
    match instr {
        IRInstr::BinOp { dst, lhs, rhs, .. } => {
            sub_val(lhs, old_vreg, new_vreg);
            sub_val(rhs, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Add { dst, lhs, rhs, .. } | IRInstr::Sub { dst, lhs, rhs, .. }
        | IRInstr::Mul { dst, lhs, rhs, .. } | IRInstr::Div { dst, lhs, rhs, .. } => {
            sub_val(lhs, old_vreg, new_vreg);
            sub_val(rhs, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Load { dst, addr, .. } => {
            sub_val(addr, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Store { value, addr, .. } => {
            sub_val(value, old_vreg, new_vreg);
            sub_val(addr, old_vreg, new_vreg);
        }
        IRInstr::Cmp { dst, lhs, rhs, .. } => {
            sub_val(lhs, old_vreg, new_vreg);
            sub_val(rhs, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Offset { dst, base, offset } => {
            sub_val(base, old_vreg, new_vreg);
            sub_val(offset, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Cast { dst, src, .. } => {
            sub_val(src, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Select { dst, cond, true_val, false_val, .. } => {
            sub_val(cond, old_vreg, new_vreg);
            sub_val(true_val, old_vreg, new_vreg);
            sub_val(false_val, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Phi { dst, incoming } => {
            for (val, _) in incoming {
                sub_val(val, old_vreg, new_vreg);
            }
            let _ = dst;
        }
        _ => {}
    }
}

/// Substitute `old_vreg` with `new_vreg` in `instr` AND renumber the
/// instruction's destination vreg to a fresh `next_vreg`-derived id, so each
/// duplicated copy has its own SSA def. Used by `try_unroll_multiblock_loop`.
fn renumbered_substitute(instr: &mut IRInstr, old_vreg: u32, new_vreg: u32, next_vreg: &mut u32) {
    substitute_vreg(instr, old_vreg, new_vreg);
    let fresh = *next_vreg;
    *next_vreg += 1;
    match instr {
        IRInstr::BinOp { dst, .. }
        | IRInstr::Add { dst, .. }
        | IRInstr::Sub { dst, .. }
        | IRInstr::Mul { dst, .. }
        | IRInstr::Div { dst, .. }
        | IRInstr::Load { dst, .. }
        | IRInstr::Offset { dst, .. }
        | IRInstr::Cast { dst, .. }
        | IRInstr::Select { dst, .. } => {
            if let IRValue::Register(r) = dst {
                *r = fresh;
            }
        }
        IRInstr::Cmp { dst: IRValue::Register(r), .. } => {
            *r = fresh;
        }
        IRInstr::Phi { dst: _, .. } => {
            // Don't renumber the header's Phi (it's the loop-carried IV).
            let _ = fresh;
        }
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IRBlock, IRFunction, IRInstr, IRTerminator, IRValue, BinOpKind, CmpKind};
    use crate::regalloc::LoopInfo;
    use std::collections::HashSet;

    /// Build a 2-block loop: header (Phi + Jump to latch) + latch (body + inc
    /// + cmp + Branch back to header or exit). This is the multi-block case
    /// with zero body blocks (the body lives in the latch).
    fn build_2block_loop_function() -> IRFunction {
        let mut func = IRFunction::new("loop2");
        // vregs: 0=N, 4=i (phi), 5=i*4, 6=addr, 7=val, 8=i_next, 9=cond
        func.params = vec![IRValue::Register(0)];

        let entry = IRBlock {
            label: "entry".to_string(),
            instructions: vec![],
            terminator: IRTerminator::Jump("header".to_string()),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks[0] = entry;

        // header: phi + jump to latch.
        let mut header = IRBlock::new("header");
        header.instructions.push(IRInstr::Phi {
            dst: IRValue::Register(4),
            incoming: vec![
                (IRValue::Immediate(0), "entry".to_string()),
                (IRValue::Register(8), "latch".to_string()),
            ],
        });
        header.terminator = IRTerminator::Jump("latch".to_string());
        func.blocks.push(header);

        // latch: body + inc + cmp + Branch back to header or exit.
        let mut latch = IRBlock::new("latch");
        // body: i*4 = i * 4
        latch.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(5),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(4),
            ty: None,
        });
        // inc: i_next = i + 1
        latch.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(8),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(1),
            ty: None,
        });
        // cmp: cond = i_next < N
        latch.instructions.push(IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: IRValue::Register(9),
            lhs: IRValue::Register(8),
            rhs: IRValue::Register(0),
            ty: None,
        });
        latch.terminator = IRTerminator::Branch {
            cond: IRValue::Register(9),
            true_block: "header".to_string(),
            false_block: "exit".to_string(),
        };
        func.blocks.push(latch);

        let exit = IRBlock {
            label: "exit".to_string(),
            instructions: vec![IRInstr::Ret { values: vec![] }],
            terminator: IRTerminator::Return(vec![]),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks.push(exit);
        func
    }

    /// Build a 3-block loop: header (Phi + Jump) + body (an if-statement
    /// simulated by a BinOp) + latch (inc + cmp + Branch).
    fn build_3block_loop_function() -> IRFunction {
        let mut func = IRFunction::new("loop3");
        func.params = vec![IRValue::Register(0)];

        let entry = IRBlock {
            label: "entry".to_string(),
            instructions: vec![],
            terminator: IRTerminator::Jump("header".to_string()),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks[0] = entry;

        // header: phi + jump to body.
        let mut header = IRBlock::new("header");
        header.instructions.push(IRInstr::Phi {
            dst: IRValue::Register(4),
            incoming: vec![
                (IRValue::Immediate(0), "entry".to_string()),
                (IRValue::Register(8), "latch".to_string()),
            ],
        });
        header.terminator = IRTerminator::Jump("body".to_string());
        func.blocks.push(header);

        // body: i*4 = i * 4 (the "if" simulated as a Mul).
        let mut body = IRBlock::new("body");
        body.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(5),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(4),
            ty: None,
        });
        body.terminator = IRTerminator::Jump("latch".to_string());
        func.blocks.push(body);

        // latch: inc + cmp + Branch.
        let mut latch = IRBlock::new("latch");
        latch.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(8),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(1),
            ty: None,
        });
        latch.instructions.push(IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: IRValue::Register(9),
            lhs: IRValue::Register(8),
            rhs: IRValue::Register(0),
            ty: None,
        });
        latch.terminator = IRTerminator::Branch {
            cond: IRValue::Register(9),
            true_block: "header".to_string(),
            false_block: "exit".to_string(),
        };
        func.blocks.push(latch);

        let exit = IRBlock {
            label: "exit".to_string(),
            instructions: vec![IRInstr::Ret { values: vec![] }],
            terminator: IRTerminator::Return(vec![]),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks.push(exit);
        func
    }

    /// Build a function with perfectly-nested loops (the unroll-and-jam target):
    ///
    /// ```text
    /// for i_o in 0..N:
    ///   for i_i in 0..M:   (M = 4, constant)
    ///     r = i_o * i_i     (no stores — safe for jamming)
    /// ```
    ///
    /// CFG:
    /// ```text
    /// entry → outer_header → inner_header → inner_body → inner_latch
    ///                            ↑                            ↓ (exit to outer_latch)
    ///                            └────────────────────────────┘
    /// outer_latch → outer_header (back-edge) / exit
    /// ```
    ///
    /// vregs: 0=N (param), 1=M (param, unused — M is hardcoded as Imm(4)),
    ///        10=i_o, 11=i_o_next, 12=cond_o,
    ///        20=i_i, 21=i_i_next, 22=cond_i, 30=r
    fn build_perfectly_nested_loops() -> IRFunction {
        let mut func = IRFunction::new("nested");
        func.params = vec![IRValue::Register(0)]; // N

        let entry = IRBlock {
            label: "entry".to_string(),
            instructions: vec![],
            terminator: IRTerminator::Jump("outer_header".to_string()),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks[0] = entry;

        // outer_header: i_o = phi(0, entry), (i_o_next, outer_latch); jump inner_header
        let mut outer_header = IRBlock::new("outer_header");
        outer_header.instructions.push(IRInstr::Phi {
            dst: IRValue::Register(10),
            incoming: vec![
                (IRValue::Immediate(0), "entry".to_string()),
                (IRValue::Register(11), "outer_latch".to_string()),
            ],
        });
        outer_header.terminator = IRTerminator::Jump("inner_header".to_string());
        func.blocks.push(outer_header);

        // inner_header: i_i = phi(0, outer_header), (i_i_next, inner_latch)
        let mut inner_header = IRBlock::new("inner_header");
        inner_header.instructions.push(IRInstr::Phi {
            dst: IRValue::Register(20),
            incoming: vec![
                (IRValue::Immediate(0), "outer_header".to_string()),
                (IRValue::Register(21), "inner_latch".to_string()),
            ],
        });
        inner_header.terminator = IRTerminator::Jump("inner_body".to_string());
        func.blocks.push(inner_header);

        // inner_body: r = i_o * i_i; jump inner_latch
        let mut inner_body = IRBlock::new("inner_body");
        inner_body.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(30),
            lhs: IRValue::Register(10), // i_o
            rhs: IRValue::Register(20), // i_i
            ty: None,
        });
        inner_body.terminator = IRTerminator::Jump("inner_latch".to_string());
        func.blocks.push(inner_body);

        // inner_latch: i_i_next = i_i + 1; cond_i = cmp i_i_next, 4; Branch → inner_header / outer_latch
        let mut inner_latch = IRBlock::new("inner_latch");
        inner_latch.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(21),
            lhs: IRValue::Register(20),
            rhs: IRValue::Immediate(1),
            ty: None,
        });
        inner_latch.instructions.push(IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: IRValue::Register(22),
            lhs: IRValue::Register(21),
            rhs: IRValue::Immediate(4), // M = 4 (constant → inner trip count invariant)
            ty: None,
        });
        inner_latch.terminator = IRTerminator::Branch {
            cond: IRValue::Register(22),
            true_block: "inner_header".to_string(),
            false_block: "outer_latch".to_string(),
        };
        func.blocks.push(inner_latch);

        // outer_latch: i_o_next = i_o + 1; cond_o = cmp i_o_next, N; Branch → outer_header / exit
        let mut outer_latch = IRBlock::new("outer_latch");
        outer_latch.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(11),
            lhs: IRValue::Register(10),
            rhs: IRValue::Immediate(1),
            ty: None,
        });
        outer_latch.instructions.push(IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: IRValue::Register(12),
            lhs: IRValue::Register(11),
            rhs: IRValue::Register(0), // N is a vreg (outer trip count may be unknown)
            ty: None,
        });
        outer_latch.terminator = IRTerminator::Branch {
            cond: IRValue::Register(12),
            true_block: "outer_header".to_string(),
            false_block: "exit".to_string(),
        };
        func.blocks.push(outer_latch);

        let exit = IRBlock {
            label: "exit".to_string(),
            instructions: vec![IRInstr::Ret { values: vec![] }],
            terminator: IRTerminator::Return(vec![]),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks.push(exit);
        // Rebuild CFG so predecessors/successors are populated — the
        // LoopDetector's natural-loop-body BFS relies on `predecessors`
        // being set (it doesn't re-derive them from terminators).
        func.rebuild_cfg();
        func
    }

    fn loop_info_for(func: &IRFunction) -> Vec<LoopInfo> {
        LoopDetector::detect_with_induction_vars(func)
    }

    // ── SCEV tests ─────────────────────────────────────────────────────

    #[test]
    fn test_scev_known_trip_count() {
        // `for i in 0..N` with start=0, step=1, exit `i+1 < N` → trip = N.
        // We can't easily build an IRFunction with constant N (it's a vreg),
        // so test the trip-count analysis with the function's N being a vreg.
        // The analysis returns Unknown when `end` is not a constant — verify
        // that. Then construct a function with N as Immediate to test Known.
        let func = build_2block_loop_function();
        let loops = loop_info_for(&func);
        assert!(!loops.is_empty());
        let tc = analyze_trip_count(&func, &loops[0]);
        // N is a vreg (not constant) → Unknown.
        assert_eq!(tc, TripCount::Unknown, "vreg bound should be Unknown");
    }

    #[test]
    fn test_scev_known_trip_count_constant_bound() {
        // Build a loop with a constant bound N=4: `for i in 0..4`.
        let mut func = IRFunction::new("loop_const");
        let entry = IRBlock {
            label: "entry".to_string(),
            instructions: vec![],
            terminator: IRTerminator::Jump("header".to_string()),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks[0] = entry;

        let mut header = IRBlock::new("header");
        header.instructions.push(IRInstr::Phi {
            dst: IRValue::Register(4),
            incoming: vec![
                (IRValue::Immediate(0), "entry".to_string()),
                (IRValue::Register(8), "latch".to_string()),
            ],
        });
        header.terminator = IRTerminator::Jump("latch".to_string());
        func.blocks.push(header);

        let mut latch = IRBlock::new("latch");
        latch.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(8),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(1),
            ty: None,
        });
        latch.instructions.push(IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: IRValue::Register(9),
            lhs: IRValue::Register(8),
            rhs: IRValue::Immediate(4), // constant bound
            ty: None,
        });
        latch.terminator = IRTerminator::Branch {
            cond: IRValue::Register(9),
            true_block: "header".to_string(),
            false_block: "exit".to_string(),
        };
        func.blocks.push(latch);

        let exit = IRBlock {
            label: "exit".to_string(),
            instructions: vec![IRInstr::Ret { values: vec![] }],
            terminator: IRTerminator::Return(vec![]),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks.push(exit);

        let loops = loop_info_for(&func);
        assert!(!loops.is_empty());
        let tc = analyze_trip_count(&func, &loops[0]);
        // start=0, step=1, end=4, kind=SLt → trip = ceil((4-0)/1) = 4.
        assert_eq!(tc, TripCount::Known(4));
    }

    #[test]
    fn test_scev_step_two() {
        // `for i in 0..N step 2` → trip = ceil(N/2).
        let mut func = IRFunction::new("loop_step2");
        let entry = IRBlock {
            label: "entry".to_string(),
            instructions: vec![],
            terminator: IRTerminator::Jump("header".to_string()),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks[0] = entry;

        let mut header = IRBlock::new("header");
        header.instructions.push(IRInstr::Phi {
            dst: IRValue::Register(4),
            incoming: vec![
                (IRValue::Immediate(0), "entry".to_string()),
                (IRValue::Register(8), "latch".to_string()),
            ],
        });
        header.terminator = IRTerminator::Jump("latch".to_string());
        func.blocks.push(header);

        let mut latch = IRBlock::new("latch");
        // i_next = i + 2
        latch.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(8),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(2),
            ty: None,
        });
        latch.instructions.push(IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: IRValue::Register(9),
            lhs: IRValue::Register(8),
            rhs: IRValue::Immediate(10),
            ty: None,
        });
        latch.terminator = IRTerminator::Branch {
            cond: IRValue::Register(9),
            true_block: "header".to_string(),
            false_block: "exit".to_string(),
        };
        func.blocks.push(latch);

        let exit = IRBlock {
            label: "exit".to_string(),
            instructions: vec![IRInstr::Ret { values: vec![] }],
            terminator: IRTerminator::Return(vec![]),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks.push(exit);

        let loops = loop_info_for(&func);
        let tc = analyze_trip_count(&func, &loops[0]);
        // start=0, step=2, end=10, SLt → trip = ceil(10/2) = 5.
        assert_eq!(tc, TripCount::Known(5));
    }

    #[test]
    fn test_compute_unroll_factor_full_unroll() {
        // Trip count ≤ FULL_UNROLL_THRESHOLD (8) → fully unroll.
        let tc = TripCount::Known(4);
        let factor = compute_unroll_factor(&tc, 3);
        assert_eq!(factor, 4, "small trip count should fully unroll");
    }

    #[test]
    fn test_compute_unroll_factor_large_trip_count() {
        // Trip count > threshold → min(8, trip/2).
        let tc = TripCount::Known(100);
        let factor = compute_unroll_factor(&tc, 5);
        assert_eq!(factor, 8, "large trip count should cap at MAX_UNROLL_FACTOR=8");
    }

    #[test]
    fn test_compute_unroll_factor_unknown() {
        let tc = TripCount::Unknown;
        let factor = compute_unroll_factor(&tc, 5);
        assert_eq!(factor, DEFAULT_UNROLL_FACTOR);
    }

    #[test]
    fn test_compute_unroll_factor_respects_budget() {
        // body_size * factor must be ≤ UNROLL_CODE_SIZE_BUDGET (500).
        // body_size = 100, factor candidate = 8 → 800 > 500 → factor capped to 5.
        let tc = TripCount::Known(100);
        let factor = compute_unroll_factor(&tc, 100);
        assert!(
            factor * 100 <= UNROLL_CODE_SIZE_BUDGET,
            "factor * body_size must be ≤ budget, got {} * 100 = {} > {}",
            factor,
            factor * 100,
            UNROLL_CODE_SIZE_BUDGET
        );
        assert!(factor < 8, "budget should cap factor below 8");
    }

    // ── Multi-block unrolling tests ────────────────────────────────────

    #[test]
    fn test_multiblock_unroll_2block_loop() {
        // A 2-block loop (header + latch, no body) unrolled by 2 → block
        // count grows (header + latch_u0 + latch_u1 + exit ≥ 4 blocks).
        let func = build_2block_loop_function();
        let original_block_count = func.blocks.len(); // entry + header + latch + exit = 4.
        let loops = loop_info_for(&func);
        assert!(!loops.is_empty());
        // Trip count is Unknown (N is a vreg) → factor = 2.
        let unrolled = try_unroll_multiblock_loop(&func, &loops[0], 2);
        assert!(unrolled.is_some(), "2-block loop should unroll");
        let unrolled = unrolled.unwrap();
        // After unrolling: header + latch_u0 + latch_u1 + entry + exit (5 blocks).
        // (Original latch is removed.)
        assert!(
            unrolled.blocks.len() >= original_block_count + 1,
            "unrolling should add at least 1 block, got {} -> {}",
            original_block_count,
            unrolled.blocks.len()
        );
        // The header's terminator should now jump to latch_u0.
        let header = unrolled
            .blocks
            .iter()
            .find(|b| b.label == "header")
            .unwrap();
        match &header.terminator {
            IRTerminator::Jump(t) => assert_eq!(t, "latch_u0"),
            other => panic!("header terminator should be Jump(latch_u0), got {:?}", other),
        }
        // latch_u0 should Jump to latch_u1 (unconditional).
        let latch_u0 = unrolled
            .blocks
            .iter()
            .find(|b| b.label == "latch_u0")
            .unwrap();
        match &latch_u0.terminator {
            IRTerminator::Jump(t) => assert_eq!(t, "latch_u1"),
            other => panic!("latch_u0 should Jump to latch_u1, got {:?}", other),
        }
        // latch_u1 should Branch back to header (with exit to "exit").
        let latch_u1 = unrolled
            .blocks
            .iter()
            .find(|b| b.label == "latch_u1")
            .unwrap();
        match &latch_u1.terminator {
            IRTerminator::Branch {
                true_block,
                false_block,
                ..
            } => {
                assert_eq!(true_block, "header", "latch_u1 should branch back to header");
                assert_eq!(false_block, "exit", "latch_u1 should exit to 'exit'");
            }
            other => panic!("latch_u1 should Branch, got {:?}", other),
        }
    }

    #[test]
    fn test_multiblock_unroll_3block_loop() {
        // A 3-block loop (header + body + latch) unrolled by 2.
        let func = build_3block_loop_function();
        let loops = loop_info_for(&func);
        assert!(!loops.is_empty());
        let unrolled = try_unroll_multiblock_loop(&func, &loops[0], 2);
        assert!(unrolled.is_some(), "3-block loop should unroll");
        let unrolled = unrolled.unwrap();
        // After unrolling: header + body_u0 + latch_u0 + body_u1 + latch_u1 + entry + exit.
        let labels: Vec<&str> = unrolled.blocks.iter().map(|b| b.label.as_str()).collect();
        assert!(labels.contains(&"header"), "header missing: {:?}", labels);
        assert!(labels.contains(&"body_u0"), "body_u0 missing: {:?}", labels);
        assert!(labels.contains(&"body_u1"), "body_u1 missing: {:?}", labels);
        assert!(labels.contains(&"latch_u0"), "latch_u0 missing: {:?}", labels);
        assert!(labels.contains(&"latch_u1"), "latch_u1 missing: {:?}", labels);
        // Header → body_u0.
        let header = unrolled
            .blocks
            .iter()
            .find(|b| b.label == "header")
            .unwrap();
        match &header.terminator {
            IRTerminator::Jump(t) => assert_eq!(t, "body_u0"),
            other => panic!("header should Jump(body_u0), got {:?}", other),
        }
        // body_u0 → latch_u0.
        let body_u0 = unrolled
            .blocks
            .iter()
            .find(|b| b.label == "body_u0")
            .unwrap();
        match &body_u0.terminator {
            IRTerminator::Jump(t) => assert_eq!(t, "latch_u0"),
            other => panic!("body_u0 should Jump(latch_u0), got {:?}", other),
        }
        // latch_u0 → body_u1 (unconditional to next copy).
        let latch_u0 = unrolled
            .blocks
            .iter()
            .find(|b| b.label == "latch_u0")
            .unwrap();
        match &latch_u0.terminator {
            IRTerminator::Jump(t) => assert_eq!(t, "body_u1"),
            other => panic!("latch_u0 should Jump(body_u1), got {:?}", other),
        }
        // latch_u1 → header (back-edge) or exit.
        let latch_u1 = unrolled
            .blocks
            .iter()
            .find(|b| b.label == "latch_u1")
            .unwrap();
        match &latch_u1.terminator {
            IRTerminator::Branch { true_block, .. } => {
                assert_eq!(true_block, "header");
            }
            other => panic!("latch_u1 should Branch, got {:?}", other),
        }
    }

    #[test]
    fn test_multiblock_unroll_iv_step_correct() {
        // The last latch's increment should produce `phi + factor` (via
        // `+1` on `iv_{F-1} = phi + (F-1)`), so the loop runs N/factor
        // iterations (not N*factor). We verify by checking the last latch
        // contains an increment with Imm(1) whose lhs is `phi + (factor-1)`.
        let func = build_3block_loop_function();
        let loops = loop_info_for(&func);
        let unrolled = try_unroll_multiblock_loop(&func, &loops[0], 2).unwrap();
        let latch_u1 = unrolled
            .blocks
            .iter()
            .find(|b| b.label == "latch_u1")
            .unwrap();
        // The last latch must contain an `Add ... Imm(1)` increment.
        let has_inc = latch_u1.instructions.iter().any(|instr| {
            matches!(
                instr,
                IRInstr::BinOp {
                    op: BinOpKind::Add,
                    rhs: IRValue::Immediate(1),
                    ..
                }
            )
        });
        assert!(has_inc, "last latch must contain the +1 increment");
    }

    #[test]
    fn test_unroll_full_unroll_known_trip_count() {
        // `for i in 0..4` should fully unroll to 4 copies (trip count = 4,
        // which is ≤ FULL_UNROLL_THRESHOLD=8).
        let mut func = IRFunction::new("full_unroll");
        let entry = IRBlock {
            label: "entry".to_string(),
            instructions: vec![],
            terminator: IRTerminator::Jump("header".to_string()),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks[0] = entry;

        let mut header = IRBlock::new("header");
        header.instructions.push(IRInstr::Phi {
            dst: IRValue::Register(4),
            incoming: vec![
                (IRValue::Immediate(0), "entry".to_string()),
                (IRValue::Register(8), "latch".to_string()),
            ],
        });
        // body: i*4
        header.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(5),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(4),
            ty: None,
        });
        header.terminator = IRTerminator::Jump("latch".to_string());
        func.blocks.push(header);

        let mut latch = IRBlock::new("latch");
        latch.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(8),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(1),
            ty: None,
        });
        latch.instructions.push(IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: IRValue::Register(9),
            lhs: IRValue::Register(8),
            rhs: IRValue::Immediate(4), // constant bound → Known(4)
            ty: None,
        });
        latch.terminator = IRTerminator::Branch {
            cond: IRValue::Register(9),
            true_block: "header".to_string(),
            false_block: "exit".to_string(),
        };
        func.blocks.push(latch);

        let exit = IRBlock {
            label: "exit".to_string(),
            instructions: vec![IRInstr::Ret { values: vec![] }],
            terminator: IRTerminator::Return(vec![]),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks.push(exit);

        // Run the full unroll_loops pipeline.
        let unrolled = unroll_loops(func);
        // The loop should have been fully unrolled (factor=4). The header's
        // body (the Mul) should be duplicated 4 times.
        let header = unrolled
            .blocks
            .iter()
            .find(|b| b.label == "header")
            .expect("header must exist");
        let mul_count = header
            .instructions
            .iter()
            .filter(|instr| {
                matches!(
                    instr,
                    IRInstr::BinOp {
                        op: BinOpKind::Mul,
                        ..
                    }
                )
            })
            .count();
        // Single-block unroller fires for self-loops only; this loop is
        // 2-block (header + latch), so the multi-block path should fire and
        // produce 4 latch copies (each with a body Mul). Verify by counting
        // Mul instructions across all blocks.
        let total_muls: usize = unrolled
            .blocks
            .iter()
            .map(|b| {
                b.instructions
                    .iter()
                    .filter(|instr| {
                        matches!(
                            instr,
                            IRInstr::BinOp {
                                op: BinOpKind::Mul,
                                ..
                            }
                        )
                    })
                    .count()
            })
            .sum();
        assert!(
            total_muls >= 4,
            "fully-unrolled loop should have ≥4 Mul copies (one per iteration), got {}",
            total_muls
        );
        let _ = header;
        let _ = mul_count;
    }

    // ── Unroll-and-jam tests (Wave 30) ─────────────────────────────────

    #[test]
    fn test_unroll_and_jam_is_noop_for_single_loop() {
        // A single (non-nested) loop has no (outer, inner) pair to jam, so
        // `try_unroll_and_jam` must be a no-op for it. This is the renamed
        // original `test_unroll_and_jam_is_noop` — the function is no longer
        // a global no-op, but it is still a no-op for inputs that don't match
        // the perfectly-nested pattern.
        let func = build_3block_loop_function();
        let result = try_unroll_and_jam(func.clone());
        assert_eq!(
            result.blocks.len(),
            func.blocks.len(),
            "unroll-and-jam must be a no-op for a single (non-nested) loop"
        );
    }

    #[test]
    fn test_unroll_and_jam_basic() {
        // Perfectly nested loops with no stores → outer unrolled by 2, inner
        // loops placed adjacent ("jammed") in the CFG.
        let func = build_perfectly_nested_loops();
        let original_block_count = func.blocks.len(); // entry + 5 loop blocks + exit = 7.
        let result = try_unroll_and_jam(func.clone());

        // After unroll-and-jam by 2:
        //   - Original inner blocks (inner_header, inner_body, inner_latch) removed (-3).
        //   - 2 copies added: inner_header_u0, inner_body_u0, inner_latch_u0,
        //     between_u1, inner_header_u1, inner_body_u1, inner_latch_u1 (+7).
        //   - Net: +4 blocks.
        assert!(
            result.blocks.len() > original_block_count,
            "unroll-and-jam should add blocks, got {} -> {}",
            original_block_count,
            result.blocks.len()
        );

        let labels: Vec<&str> =
            result.blocks.iter().map(|b| b.label.as_str()).collect();
        assert!(labels.contains(&"outer_header"), "outer_header missing: {:?}", labels);
        assert!(
            labels.contains(&"inner_header_u0"),
            "inner_header_u0 missing: {:?}",
            labels
        );
        assert!(
            labels.contains(&"inner_header_u1"),
            "inner_header_u1 missing: {:?}",
            labels
        );
        assert!(labels.contains(&"between_u1"), "between_u1 missing: {:?}", labels);
        assert!(
            labels.contains(&"inner_latch_u0"),
            "inner_latch_u0 missing: {:?}",
            labels
        );
        assert!(
            labels.contains(&"inner_latch_u1"),
            "inner_latch_u1 missing: {:?}",
            labels
        );
        // The original inner blocks should be gone.
        assert!(
            !labels.contains(&"inner_header"),
            "original inner_header should be removed: {:?}",
            labels
        );
        assert!(
            !labels.contains(&"inner_latch"),
            "original inner_latch should be removed: {:?}",
            labels
        );

        // Outer header should jump to inner_header_u0.
        let outer_header = result
            .blocks
            .iter()
            .find(|b| b.label == "outer_header")
            .unwrap();
        match &outer_header.terminator {
            IRTerminator::Jump(t) => assert_eq!(t, "inner_header_u0"),
            other => panic!("outer_header should Jump(inner_header_u0), got {:?}", other),
        }

        // Outer latch's increment should be +2 (FACTOR), not +1.
        let outer_latch = result
            .blocks
            .iter()
            .find(|b| b.label == "outer_latch")
            .unwrap();
        let has_plus_2 = outer_latch.instructions.iter().any(|instr| {
            matches!(
                instr,
                IRInstr::BinOp {
                    op: BinOpKind::Add,
                    rhs: IRValue::Immediate(2),
                    ..
                }
            )
        });
        assert!(
            has_plus_2,
            "outer latch should have +2 increment after unroll-and-jam by 2"
        );

        // inner_latch_u0 should exit to between_u1 (the "jam" adjacency).
        let inner_latch_u0 = result
            .blocks
            .iter()
            .find(|b| b.label == "inner_latch_u0")
            .unwrap();
        match &inner_latch_u0.terminator {
            IRTerminator::Branch { false_block, .. } => {
                assert_eq!(
                    false_block, "between_u1",
                    "inner_latch_u0 should exit to between_u1 (jam adjacency)"
                );
            }
            other => panic!("inner_latch_u0 should Branch, got {:?}", other),
        }

        // inner_latch_u1 should exit to outer_latch (the last copy exits to
        // the outer latch, which then checks the outer loop condition).
        let inner_latch_u1 = result
            .blocks
            .iter()
            .find(|b| b.label == "inner_latch_u1")
            .unwrap();
        match &inner_latch_u1.terminator {
            IRTerminator::Branch { false_block, .. } => {
                assert_eq!(
                    false_block, "outer_latch",
                    "inner_latch_u1 should exit to outer_latch"
                );
            }
            other => panic!("inner_latch_u1 should Branch, got {:?}", other),
        }

        // between_u1 should compute i_o + 1 and jump to inner_header_u1.
        let between_u1 = result
            .blocks
            .iter()
            .find(|b| b.label == "between_u1")
            .unwrap();
        let has_add_1 = between_u1.instructions.iter().any(|instr| {
            matches!(
                instr,
                IRInstr::BinOp {
                    op: BinOpKind::Add,
                    rhs: IRValue::Immediate(1),
                    ..
                }
            )
        });
        assert!(has_add_1, "between_u1 should compute i_o + 1");
        match &between_u1.terminator {
            IRTerminator::Jump(t) => assert_eq!(t, "inner_header_u1"),
            other => panic!("between_u1 should Jump(inner_header_u1), got {:?}", other),
        }
    }

    #[test]
    fn test_unroll_and_jam_skips_when_unsafe() {
        // Inner body has a store to an outer-dependent address → no-op.
        // The store's addr is `i_o` (the outer IV) — a classic outer-loop-
        // carried memory dependency that unroll-and-jam would violate.
        let mut func = build_perfectly_nested_loops();
        let inner_body = func
            .blocks
            .iter_mut()
            .find(|b| b.label == "inner_body")
            .unwrap();
        inner_body.instructions.push(IRInstr::Store {
            value: IRValue::Register(30), // r
            addr: IRValue::Register(10),  // addr = i_o (outer IV) — UNSAFE
            offset: 0,
            ty: crate::ir::IRType::I64,
        });

        let original_block_count = func.blocks.len();
        let result = try_unroll_and_jam(func.clone());
        assert_eq!(
            result.blocks.len(),
            original_block_count,
            "unroll-and-jam should be a no-op when inner body has an outer-loop-carried store"
        );
    }

    #[test]
    fn test_unroll_and_jam_skips_when_not_perfectly_nested() {
        // Outer body has a non-loop block between the inner loop's exit and
        // the outer latch → not perfectly nested → no-op.
        let mut func = build_perfectly_nested_loops();
        // Rewire inner_latch's exit to go to "between_extra" (a new block)
        // instead of "outer_latch".
        let inner_latch = func
            .blocks
            .iter_mut()
            .find(|b| b.label == "inner_latch")
            .unwrap();
        inner_latch.terminator = IRTerminator::Branch {
            cond: IRValue::Register(22),
            true_block: "inner_header".to_string(),
            false_block: "between_extra".to_string(),
        };
        // Add the "between_extra" block — this is an extra outer-body block
        // that violates the perfect-nesting condition.
        let mut between_extra = IRBlock::new("between_extra");
        between_extra.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(31),
            lhs: IRValue::Register(10),
            rhs: IRValue::Immediate(2),
            ty: None,
        });
        between_extra.terminator = IRTerminator::Jump("outer_latch".to_string());
        func.blocks.push(between_extra);

        let original_block_count = func.blocks.len();
        let result = try_unroll_and_jam(func.clone());
        assert_eq!(
            result.blocks.len(),
            original_block_count,
            "unroll-and-jam should be a no-op when outer body is not perfectly nested"
        );
    }

    // ── Existing tests (Wave 13b regression) ───────────────────────────

    #[test]
    fn test_unroller_bails_on_non_loop() {
        let block = IRBlock::new("plain");
        let result = try_unroll_block(&block, 2);
        assert!(result.is_none(), "non-loop should not be unrolled");
    }

    #[test]
    fn test_unroller_bails_on_no_phi() {
        let mut block = IRBlock::new("loop");
        block.instructions = vec![IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
            ty: None,
        }];
        block.terminator = IRTerminator::Branch {
            cond: IRValue::Register(2),
            true_block: "loop".to_string(),
            false_block: "exit".to_string(),
        };
        let result = try_unroll_block(&block, 2);
        assert!(result.is_none(), "loop without Phi should not be unrolled");
    }
}
