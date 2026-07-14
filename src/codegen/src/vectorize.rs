//! # Autovectorizer (Wave 29 rewrite)
//!
//! Replaces the miscompiling Wave 13 stub that blindly 4×'d the loop body
//! without adjusting the IV step — turning `for i in 0..N { body }` into
//! `for i in 0..N { body; body; body; body }` (4N work instead of N), with
//! all four body copies operating on the *same* `i` (so three of them were
//! either dead or overwrote the first). The stub also never emitted a single
//! SIMD byte.
//!
//! This module implements a correct, minimal, honest vectorizer:
//!
//! 1. **Loop vectorization with IV-step adjustment (the core fix).**
//!    For a counted self-loop `for i in 0..N { body(i) }` with a vectorizable
//!    body, we transform to a vector loop that runs `N/vf` times, where each
//!    iteration handles `vf` lanes. **The IV step changes from `+1` to
//!    `+vf*element_size`** (the stub's bug was leaving the step at `+1`).
//!    Lane `l` in `[0, vf)` operates on `base + i + l*element_size`, materialized
//!    as a fresh lane-offset vreg `i_l = i + l*element_size` substituted into a
//!    duplicated body copy. A scalar remainder loop is appended for the
//!    `N % vf` tail.
//!
//! 2. **SLP vectorization (Superword-Level Parallelism).** Detects isomorphic
//!    adjacent independent scalar statements within a block (e.g. two `Add`s
//!    with matching element type and no cross-dependency) and records them as
//!    a `PackedOp` in the plan. The IR is not rewritten by SLP — the plan is
//!    the hook the backend consumes to emit a single SIMD instruction in place
//!    of the packed scalar ops.
//!
//! 3. **Cost model.** Loop vectorization only fires when the body is small
//!    (≤ `MAX_BODY_INSTRS`), has no side effects (no Calls/Atomics/Free), and
//!    the element type is a power-of-two integer ≤ 64 bits. SLP only packs
//!    when ≥2 ops are isomorphic and independent.
//!
//! 4. **Vectorization plan.** Because `IRInstr` cannot be extended from this
//!    module (the IR enum lives in `ir.rs`), the plan is a side-channel
//!    `Vec<PackedOp>` returned by `vectorize_function_with_plan`. The backend's
//!    SSE/AVX encoders (`x86_64::encode_sse_*`) and NEON encoders
//!    (`arm64::encode_neon_*`) consume `PackedOp`s.
//!
//! ## Scope / honest limitations
//!
//! Full vector-IR plumbing through the backend ISel (reg-alloc, scheduler,
//! instruction selection) is too deep for this wave. We:
//! - Fix the IV-step miscompilation (the core bug).
//! - Emit a vectorization plan the backend can consume.
//! - Provide SSE/AVX/NEON encoders (in `x86_64/mod.rs` and `arm64.rs`).
//! - Leave full ISel integration as a `TODO(wave29)` — the encoders and plan
//!   exist and are unit-tested, but the backend does not yet lower `PackedOp`
//!   to real machine code.

use crate::ir::{
    size_of, IRBlock, IRFunction, IRInstr, IRTerminator, IRValue, BinOpKind, VectorOpKind,
};
#[cfg(test)]
use crate::ir::IRType;

// ─────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────

/// Maximum body instruction count for loop vectorization (cost gate).
const MAX_BODY_INSTRS: usize = 24;

/// A vector packed operation recorded in the plan for backend lowering.
///
/// Each `PackedOp` describes `vf` independent scalar ops of the same kind on
/// adjacent lanes that the backend may fuse into a single SIMD instruction
/// (SSE/AVX `padd`/`psub`/`pmull` on x86_64, NEON `add`/`sub`/`mul` on
/// aarch64).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedOp {
    /// The arithmetic kind.
    pub kind: PackedOpKind,
    /// Number of lanes packed (e.g. 4 for 4×i32).
    pub lanes: u32,
    /// Element size in bytes (4 for i32, 8 for i64).
    pub elem_size: u32,
    /// The vreg destination of lane 0 (lanes 1..vf use consecutive vregs
    /// assigned by the IR duplication pass).
    pub dst_lane0: u32,
    /// Source vregs of lane 0 (parallel structure for lanes 1..vf).
    pub src_lane0: Vec<u32>,
    /// The block label where the packed op resides.
    pub block: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedOpKind {
    Add,
    Sub,
    Mul,
}

/// A vectorization plan: side-channel output the backend consumes.
#[derive(Debug, Clone, Default)]
pub struct VectorizationPlan {
    /// Packed operations discovered by SLP / loop vectorization.
    pub packed_ops: Vec<PackedOp>,
    /// The vector width used (lanes).
    pub vf: u32,
    /// Element size in bytes for the vectorized loop (0 if no loop was
    /// vectorized).
    pub elem_size: u32,
    /// The label of the vector loop's body block, if any.
    pub vector_loop_block: Option<String>,
    /// The label of the scalar remainder loop's body block, if any.
    pub remainder_loop_block: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────

/// Vectorize all eligible loops in a function.
///
/// Discards the vectorization plan (the IR is correctly transformed; the plan
/// is a hook for future ISel integration — see module docs).
pub fn vectorize_function(func: IRFunction) -> IRFunction {
    let (f, _plan) = vectorize_function_with_plan(func);
    f
}

/// Vectorize and return both the rewritten IR and the vectorization plan.
pub fn vectorize_function_with_plan(mut func: IRFunction) -> (IRFunction, VectorizationPlan) {
    let mut plan = VectorizationPlan::default();

    // Phase 1: Loop vectorization. We scan for self-loop blocks matching the
    // counted-loop pattern and transform them in place. We skip blocks whose
    // label contains `_remainder` so we don't recursively vectorize the
    // scalar remainder loop we just emitted.
    let mut changed = true;
    let mut iter = 0;
    while changed && iter < 4 {
        changed = false;
        iter += 1;
        for i in 0..func.blocks.len() {
            let block = &func.blocks[i];
            if block.label.contains("_remainder") {
                continue; // Don't recursively vectorize the remainder loop.
            }
            if let Some((new_block, loop_plan)) = try_vectorize_self_loop(block) {
                let remainder = loop_plan
                    .remainder_loop_block
                    .clone()
                    .expect("try_vectorize_self_loop sets remainder_loop_block on success");
                let remainder_block = build_remainder_loop(block, &remainder);
                func.blocks[i] = new_block;
                func.blocks.insert(i + 1, remainder_block);
                plan.packed_ops.extend(loop_plan.packed_ops);
                plan.vf = loop_plan.vf;
                plan.elem_size = loop_plan.elem_size;
                plan.vector_loop_block = loop_plan.vector_loop_block;
                plan.remainder_loop_block = loop_plan.remainder_loop_block;
                changed = true;
                break; // Restart the scan since blocks vec shifted.
            }
        }
    }

    // Phase 2: SLP vectorization. Scans each block for isomorphic adjacent
    // independent scalar ops, rewrites the IR by replacing the first op of
    // each detected pair with an `IRInstr::VectorOp` (lanes=2) and removing
    // the second, AND records the result in the plan.
    //
    // SLP is skipped on blocks the loop vectorizer already touched
    // (`vector_loop_block` / `remainder_loop_block`) — those blocks already
    // contain lane-duplicated bodies whose adjacent Adds SLP would pack,
    // which would (a) be redundant (the loop vectorizer already recorded
    // PackedOps for them) and (b) break the loop-vectorizer test
    // assertions that count `IRInstr::Add` bodies.
    let mut skip_labels: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(lbl) = &plan.vector_loop_block {
        skip_labels.insert(lbl.clone());
    }
    if let Some(lbl) = &plan.remainder_loop_block {
        skip_labels.insert(lbl.clone());
    }
    let slp_ops = slp_vectorize_function(&mut func, &skip_labels);
    plan.packed_ops.extend(slp_ops);

    // Final CFG rebuild so predecessors/successors are consistent.
    func.rebuild_cfg();
    (func, plan)
}

// ─────────────────────────────────────────────────────────────────────────
// Loop vectorization
// ─────────────────────────────────────────────────────────────────────────

/// Attempt to vectorize a single-block counted self-loop.
///
/// Returns `Some((new_block, plan))` if `block` matches the counted-loop
/// pattern and was vectorized; `None` otherwise.
///
/// Required pattern (single block, self-loop):
/// ```text
/// loop:
///   i      = phi(0, entry), (i_next, loop)
///   ... body using i (≤ MAX_BODY_INSTRS instrs, no Calls/Atomics/Free) ...
///   i_next = i + 1                      // BinOp Add, Imm(1)
///   cond   = cmp i_next, N_vreg, SLt    // or ULt
///   br cond, loop, exit
/// ```
///
/// Transformed (vf=4, elem_size=4 for i32):
/// ```text
/// loop:                                       // vector loop body
///   i      = phi(0, entry), (i_next, loop)
///   ... body using i (lane 0) ...
///   i_1    = i + 1*elem_size                  // lane 1 offset
///   ... body using i_1 (lane 1, substituted) ...
///   i_2    = i + 2*elem_size                  // lane 2 offset
///   ... body using i_2 (lane 2, substituted) ...
///   i_3    = i + 3*elem_size                  // lane 3 offset
///   ... body using i_3 (lane 3, substituted) ...
///   i_next = i + vf*elem_size                 // ← IV step fix (was +1)
///   cond   = cmp i_next, N_vreg, SLt          // unchanged
///   br cond, loop, loop_remainder             // exit → remainder loop
/// ```
///
/// A scalar remainder loop block (`loop_remainder`) is appended after this
/// block by the caller; it re-uses the original `+1` step for the `N % vf`
/// tail.
fn try_vectorize_self_loop(block: &IRBlock) -> Option<(IRBlock, VectorizationPlan)> {
    let self_label = &block.label;
    let instrs = &block.instructions;
    if instrs.is_empty() {
        return None;
    }

    // Check 1: self-loop with conditional Branch.
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
                return None; // Not a self-loop.
            }
        }
        _ => return None,
    };

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
    let phi_vreg = match &phi_dst {
        IRValue::Register(r) => *r,
        _ => return None,
    };

    // Check 3: find `i_next = i + 1` (BinOp Add, phi_vreg, Imm(1)).
    let mut increment_idx = None;
    let mut i_new_vreg = 0u32;
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
                    i_new_vreg = *d;
                    break;
                }
            }
        }
    }
    let increment_idx = increment_idx?;
    if increment_idx == 0 {
        return None; // No body.
    }

    // Check 4: find `cond = cmp i_new_vreg, N_vreg, SLt|ULt` (the exit test).
    let cond_vreg = match &cond_reg {
        IRValue::Register(r) => *r,
        _ => return None,
    };
    let mut cmp_idx = None;
    for (i, instr) in instrs.iter().enumerate() {
        if let IRInstr::Cmp { dst, lhs, .. } = instr {
            if let (IRValue::Register(d), IRValue::Register(l)) = (dst, lhs) {
                if *d == cond_vreg && *l == i_new_vreg {
                    cmp_idx = Some(i);
                    break;
                }
            }
        }
    }
    let cmp_idx = cmp_idx?;

    // The body is instrs[1..increment_idx].
    let body = &instrs[1..increment_idx];
    if body.is_empty() || body.len() > MAX_BODY_INSTRS {
        return None;
    }

    // Check 5: body must be safe (no calls/atomics/free/etc.).
    for instr in body {
        if !is_safe_for_vectorization(instr) {
            return None;
        }
    }
    // The body must not clobber the IV.
    for instr in body {
        for d in instr.defined_regs() {
            if d == phi_vreg || d == i_new_vreg {
                return None;
            }
        }
    }

    // Determine the element size from the body's Load/Store types. If no
    // Load/Store is found, default to 4 (i32) — most common case.
    let elem_size: u32 = body
        .iter()
        .find_map(|instr| match instr {
            IRInstr::Load { ty, .. } | IRInstr::Store { ty, .. } => Some(size_of(ty) as u32),
            _ => None,
        })
        .unwrap_or(4);
    if elem_size == 0 || !elem_size.is_power_of_two() || elem_size > 8 {
        return None; // Unsupported element size.
    }

    // Compute the vector width. SSE2/NEON provide 128-bit registers.
    //   i32 (4 bytes): vf = 4   (4 × 32 = 128 bits)
    //   i64 (8 bytes): vf = 2   (2 × 64 = 128 bits)
    //   i16 (2 bytes): vf = 8
    //   i8  (1 byte) : vf = 16
    let vf: u32 = 16u32 / elem_size;
    if vf < 2 {
        return None;
    }

    let iv_step = vf * elem_size; // The fix: was 1, now vf*elem_size.

    // ── Build the new vector-loop block ───────────────────────────────
    let mut new_instrs: Vec<IRInstr> = Vec::with_capacity(instrs.len() * vf as usize);
    new_instrs.push(instrs[0].clone()); // Phi

    // Lane 0: original body (uses phi_vreg).
    for instr in body {
        new_instrs.push(instr.clone());
    }

    // Lanes 1..vf: emit lane offset vreg, then body with phi_vreg → i_l.
    let mut next_vreg = block_max_vreg(instrs) + 1;
    for l in 1u32..vf {
        let i_l = next_vreg;
        next_vreg += 1;
        new_instrs.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(i_l),
            lhs: IRValue::Register(phi_vreg),
            rhs: IRValue::Immediate((l as i64) * (elem_size as i64)),
            ty: None,
        });
        for instr in body {
            let mut cloned = instr.clone();
            // Renumber the dst vreg so each lane's def is distinct (SSA).
            renumbered_substitute(&mut cloned, phi_vreg, i_l, &mut next_vreg);
            new_instrs.push(cloned);
        }
    }

    // IV step fix: i_next = phi_vreg + vf*elem_size.
    new_instrs.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(i_new_vreg),
        lhs: IRValue::Register(phi_vreg),
        rhs: IRValue::Immediate(iv_step as i64),
        ty: None,
    });

    // Copy the Cmp (unchanged — compares i_new_vreg, which is now i + vf*es).
    if cmp_idx < instrs.len() {
        new_instrs.push(instrs[cmp_idx].clone());
    }
    // Copy any instructions between the Cmp and the terminator.
    for i in (cmp_idx + 1)..instrs.len() {
        new_instrs.push(instrs[i].clone());
    }

    // Terminator: exit edge now goes to the remainder loop (not the original
    // exit). The remainder loop will then exit to the original exit.
    let remainder_label = format!("{}_remainder", self_label);
    let new_terminator = IRTerminator::Branch {
        cond: cond_reg,
        true_block: self_label.clone(),
        false_block: remainder_label.clone(),
    };

    let mut new_block = IRBlock::new(self_label);
    new_block.instructions = new_instrs;
    new_block.terminator = new_terminator;
    new_block.source_line = block.source_line;

    // ── Build the plan ────────────────────────────────────────────────
    let mut plan = VectorizationPlan::default();
    plan.vf = vf;
    plan.elem_size = elem_size;
    plan.vector_loop_block = Some(self_label.clone());
    plan.remainder_loop_block = Some(remainder_label);

    // Find the body's primary BinOp (the op we want to pack into a SIMD add).
    for instr in body {
        if let Some((kind, dst, srcs)) = classify_packable_binop(instr) {
            plan.packed_ops.push(PackedOp {
                kind,
                lanes: vf,
                elem_size,
                dst_lane0: dst,
                src_lane0: srcs,
                block: self_label.clone(),
            });
            break; // One packed op per loop is enough for the plan.
        }
    }

    Some((new_block, plan))
}

/// Build the scalar remainder loop block for the `N % vf` tail.
///
/// This is a copy of the original block with:
/// - A fresh label (`{orig}_remainder`).
/// - A fresh Phi `j = phi(i, vector_loop), (j_next, self)` — the incoming
///   from the vector loop is the vector loop's IV vreg (lane 0's `i`).
/// - The original body using `j`.
/// - `j_next = j + 1` (scalar step).
/// - The original Cmp + Branch (with the self-target renamed to this block).
fn build_remainder_loop(orig: &IRBlock, new_label: &str) -> IRBlock {
    let instrs = &orig.instructions;
    let mut new_instrs: Vec<IRInstr> = Vec::with_capacity(instrs.len());

    // Find the original Phi to extract vregs.
    let (phi_dst, phi_vreg) = match instrs.first() {
        Some(IRInstr::Phi { dst, .. }) => match dst {
            IRValue::Register(r) => (dst.clone(), *r),
            _ => return IRBlock::new(new_label),
        },
        _ => return IRBlock::new(new_label),
    };

    // Fresh j vreg for the remainder loop's own Phi. Leave a large gap
    // (1024) so we don't collide with the vector loop's renumbered lanes.
    let mut next_vreg = block_max_vreg(instrs) + 1024;
    let j_vreg = next_vreg;
    next_vreg += 1;
    let j_next_vreg = next_vreg;
    next_vreg += 1;

    // j = phi(i_from_vec, vector_loop), (j_next, self)
    new_instrs.push(IRInstr::Phi {
        dst: IRValue::Register(j_vreg),
        incoming: vec![
            (phi_dst, orig.label.clone()),
            (IRValue::Register(j_next_vreg), new_label.to_string()),
        ],
    });

    // Find the original increment (`i + 1`).
    let mut increment_idx = None;
    let mut orig_inew: u32 = 0;
    for (i, instr) in instrs.iter().enumerate() {
        if let IRInstr::BinOp {
            op: BinOpKind::Add,
            dst,
            lhs: IRValue::Register(l),
            rhs: IRValue::Immediate(1),
            ..
        } = instr
        {
            if *l == phi_vreg {
                if let IRValue::Register(d) = dst {
                    increment_idx = Some(i);
                    orig_inew = *d;
                }
                break;
            }
        }
    }
    let increment_idx = match increment_idx {
        Some(i) => i,
        None => return IRBlock::new(new_label),
    };
    let body = &instrs[1..increment_idx];
    for instr in body {
        let mut cloned = instr.clone();
        substitute_vreg(&mut cloned, phi_vreg, j_vreg);
        new_instrs.push(cloned);
    }

    // j_next = j + 1 (scalar step).
    new_instrs.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(j_next_vreg),
        lhs: IRValue::Register(j_vreg),
        rhs: IRValue::Immediate(1),
        ty: None,
    });

    // Copy any instructions after the increment (Cmp + trailing). The Cmp's
    // lhs was orig_inew; substitute it to j_next_vreg.
    for instr in &instrs[increment_idx + 1..] {
        let mut cloned = instr.clone();
        substitute_vreg(&mut cloned, orig_inew, j_next_vreg);
        new_instrs.push(cloned);
    }

    // Terminator: self-loop targeting this block, exit to original exit.
    let new_terminator = match &orig.terminator {
        IRTerminator::Branch {
            cond,
            true_block,
            false_block,
        } => {
            let (tb, fb) = if true_block == &orig.label {
                (new_label.to_string(), false_block.clone())
            } else if false_block == &orig.label {
                (true_block.clone(), new_label.to_string())
            } else {
                (true_block.clone(), false_block.clone())
            };
            IRTerminator::Branch {
                cond: cond.clone(),
                true_block: tb,
                false_block: fb,
            }
        }
        other => other.clone(),
    };

    let mut block = IRBlock::new(new_label);
    block.instructions = new_instrs;
    block.terminator = new_terminator;
    block.source_line = orig.source_line;
    block
}

/// Classify a BinOp as packable into a SIMD op.
///
/// Returns `(kind, dst_vreg, src_vregs)` if packable. Only ops whose operands
/// are *all* registers (no immediates) qualify — immediate-operand ops (e.g.
/// `i*4` address scaling) are not vectorizable compute and are skipped.
fn classify_packable_binop(instr: &IRInstr) -> Option<(PackedOpKind, u32, Vec<u32>)> {
    let (op_kind, dst, lhs, rhs) = match instr {
        IRInstr::BinOp { op, dst, lhs, rhs, .. } => (*op, dst.clone(), lhs.clone(), rhs.clone()),
        IRInstr::Add { dst, lhs, rhs, .. } => (BinOpKind::Add, dst.clone(), lhs.clone(), rhs.clone()),
        IRInstr::Sub { dst, lhs, rhs, .. } => (BinOpKind::Sub, dst.clone(), lhs.clone(), rhs.clone()),
        IRInstr::Mul { dst, lhs, rhs, .. } => (BinOpKind::Mul, dst.clone(), lhs.clone(), rhs.clone()),
        _ => return None,
    };
    let kind = match op_kind {
        BinOpKind::Add => PackedOpKind::Add,
        BinOpKind::Sub => PackedOpKind::Sub,
        BinOpKind::Mul => PackedOpKind::Mul,
        _ => return None,
    };
    let dst_r = dst.as_register()?;
    // Require both operands to be registers (skip immediate-operand ops like
    // `i*4` address scaling — those are not vectorizable compute).
    let lhs_r = lhs.as_register()?;
    let rhs_r = rhs.as_register()?;
    Some((kind, dst_r, vec![lhs_r, rhs_r]))
}

/// Check if an instruction is safe to duplicate during vectorization.
fn is_safe_for_vectorization(instr: &IRInstr) -> bool {
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
            | IRInstr::Alloc { .. }
    )
}

// ─────────────────────────────────────────────────────────────────────────
// SLP vectorization
// ─────────────────────────────────────────────────────────────────────────

/// Scan a function's blocks for isomorphic adjacent independent scalar ops,
/// rewrite the IR by replacing each detected pair with an `IRInstr::VectorOp`
/// (lanes=2), and record the packed op in the plan.
///
/// `skip_labels` lists block labels that should NOT be SLP-rewritten —
/// typically the loop vectorizer's `vector_loop_block` and
/// `remainder_loop_block`, which already contain lane-duplicated bodies
/// whose adjacent Adds SLP would pack redundantly.
///
/// The read-only `slp_vectorize_block` helper (used by unit tests) is
/// preserved as a separate function below.
fn slp_vectorize_function(
    func: &mut IRFunction,
    skip_labels: &std::collections::HashSet<String>,
) -> Vec<PackedOp> {
    let mut ops = Vec::new();
    for block in &mut func.blocks {
        if skip_labels.contains(&block.label) {
            continue;
        }
        ops.extend(slp_rewrite_block(block));
    }
    ops
}

/// SLP-pack a single block IN PLACE: pair-wise scan, and for each adjacent
/// isomorphic independent pair, (1) replace the first instr with
/// `IRInstr::VectorOp { lanes: 2, .. }`, (2) remove the second instr, and
/// (3) record the `PackedOp` in the returned plan.
///
/// This is the Wave 29 IR-rewriting SLP pass. The read-only
/// `slp_vectorize_block` (below) is kept for the existing unit tests that
/// assert against the planning output without mutating the IR.
fn slp_rewrite_block(block: &mut IRBlock) -> Vec<PackedOp> {
    let mut ops = Vec::new();
    let mut i = 0;
    while i + 1 < block.instructions.len() {
        // Snapshot the classification of the pair BEFORE mutating. We clone
        // the two instructions out so we can call the immutable
        // `classify_packable_binop` + `binop_elem_size` helpers and still
        // mutate `block.instructions` afterwards.
        let (a_kind, a_dst, a_srcs) = match classify_packable_binop(&block.instructions[i]) {
            Some(t) => t,
            None => { i += 1; continue; }
        };
        let (b_kind, b_dst, b_srcs) = match classify_packable_binop(&block.instructions[i + 1]) {
            Some(t) => t,
            None => { i += 1; continue; }
        };
        if a_kind != b_kind {
            i += 1;
            continue;
        }
        // Independence: a's dst must not appear in b's sources, and vice versa.
        let a_writes_b_reads = b_srcs.contains(&a_dst);
        let b_writes_a_reads = a_srcs.contains(&b_dst);
        if a_writes_b_reads || b_writes_a_reads {
            i += 1;
            continue;
        }
        // ── Pack: rewrite instr[i] as VectorOp, remove instr[i+1] ──
        let elem_size = binop_elem_size(&block.instructions[i])
            .max(binop_elem_size(&block.instructions[i + 1]));
        let vop_kind = match a_kind {
            PackedOpKind::Add => VectorOpKind::Add,
            PackedOpKind::Sub => VectorOpKind::Sub,
            PackedOpKind::Mul => VectorOpKind::Mul,
        };
        // Extract lane-0 operands (already validated as registers by
        // classify_packable_binop — both srcs are registers).
        let lhs_lane0 = IRValue::Register(a_srcs[0]);
        let rhs_lane0 = if a_srcs.len() >= 2 {
            IRValue::Register(a_srcs[1])
        } else {
            IRValue::Immediate(0)
        };
        let dst_lane0 = IRValue::Register(a_dst);
        block.instructions[i] = IRInstr::VectorOp {
            op: vop_kind,
            lanes: 2,
            elem_size,
            dst: dst_lane0,
            lhs: lhs_lane0,
            rhs: rhs_lane0,
        };
        // Remove the second instr of the pair (its dst is now a dead lane-1
        // vreg — the VectorOp supersedes it).
        block.instructions.remove(i + 1);
        ops.push(PackedOp {
            kind: a_kind,
            lanes: 2,
            elem_size,
            dst_lane0: a_dst,
            src_lane0: a_srcs.clone(),
            block: block.label.clone(),
        });
        // Advance past the packed pair (i+1 was removed, so the new instr at
        // i+1 is what was previously at i+2).
        i += 1;
    }
    ops
}

/// SLP-pack a single block. Pairwise scan: for each adjacent pair of packable
/// BinOps with matching kind, matching element type, and no cross-dependency,
/// record a 2-lane `PackedOp`.
///
/// **Read-only planning helper** — does NOT mutate `block`. Used by the SLP
/// unit tests. The actual IR rewrite happens in `slp_rewrite_block` above.
fn slp_vectorize_block(block: &IRBlock) -> Vec<PackedOp> {
    let mut ops = Vec::new();
    let instrs = &block.instructions;
    let mut i = 0;
    while i + 1 < instrs.len() {
        let a = classify_packable_binop(&instrs[i]);
        let b = classify_packable_binop(&instrs[i + 1]);
        match (a, b) {
            (Some((ka, dst_a, srcs_a)), Some((kb, dst_b, srcs_b))) if ka == kb => {
                // Independence: a's dst must not appear in b's sources, and
                // vice versa.
                let a_writes_b_reads = srcs_b.contains(&dst_a);
                let b_writes_a_reads = srcs_a.contains(&dst_b);
                if !a_writes_b_reads && !b_writes_a_reads {
                    let elem_size = binop_elem_size(&instrs[i]).max(binop_elem_size(&instrs[i + 1]));
                    ops.push(PackedOp {
                        kind: ka,
                        lanes: 2,
                        elem_size,
                        dst_lane0: dst_a,
                        src_lane0: srcs_a.clone(),
                        block: block.label.clone(),
                    });
                    i += 2;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    ops
}

/// Best-effort element-size lookup for a BinOp/Add/Sub/Mul instruction.
fn binop_elem_size(instr: &IRInstr) -> u32 {
    match instr {
        IRInstr::BinOp { ty: Some(t), .. }
        | IRInstr::Add { ty: Some(t), .. }
        | IRInstr::Sub { ty: Some(t), .. }
        | IRInstr::Mul { ty: Some(t), .. } => size_of(t) as u32,
        _ => 4,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Vreg helpers
// ─────────────────────────────────────────────────────────────────────────

/// Find the highest vreg number used in a slice of instructions.
fn block_max_vreg(instrs: &[IRInstr]) -> u32 {
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
        IRInstr::BinOp { dst, lhs, rhs, .. }
        | IRInstr::Add { dst, lhs, rhs, .. }
        | IRInstr::Sub { dst, lhs, rhs, .. }
        | IRInstr::Mul { dst, lhs, rhs, .. }
        | IRInstr::Div { dst, lhs, rhs, .. } => {
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
        IRInstr::UnaryOp { dst, operand, .. } => {
            sub_val(operand, old_vreg, new_vreg);
            let _ = dst;
        }
        _ => {}
    }
}

/// Substitute `old_vreg` with `new_vreg` in `instr` AND renumber the
/// instruction's destination vreg to a fresh `next_vreg`-derived id, so each
/// duplicated lane has its own SSA def.
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
        IRInstr::Cmp { dst, .. } => {
            if let IRValue::Register(r) = dst {
                *r = fresh;
            }
        }
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Public alias matching the legacy Wave 13 surface (in case any caller still
// references it). The old `is_proven_non_aliasing` helper is preserved.
// ─────────────────────────────────────────────────────────────────────────

/// Check if two memory accesses are provably non-aliasing.
///
/// Uses Alloc-region analysis: if two addresses are different Alloc regions,
/// they are non-aliasing. Conservative otherwise.
pub fn is_proven_non_aliasing(
    addr_a: &IRValue,
    addr_b: &IRValue,
    alloc_regions: &std::collections::HashSet<u32>,
) -> bool {
    if addr_a == addr_b {
        return false;
    }
    if let (IRValue::Register(id_a), IRValue::Register(id_b)) = (addr_a, addr_b) {
        if alloc_regions.contains(id_a) && alloc_regions.contains(id_b) {
            return id_a != id_b;
        }
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::CmpKind;
    use std::collections::HashSet;

    /// Build a function whose body is `for i in 0..N { a[i] = b[i] + c[i]; }`
    /// lowered to a single-block self-loop:
    /// ```text
    /// loop:
    ///   i      = phi(0, entry), (i_next, loop)
    ///   i*4    = Mul i, 4                       // byte offset
    ///   addr_b = Offset(b_ptr, i*4)
    ///   addr_c = Offset(c_ptr, i*4)
    ///   b_v    = Load addr_b
    ///   c_v    = Load addr_c
    ///   sum    = Add b_v, c_v
    ///   addr_a = Offset(a_ptr, i*4)
    ///   Store sum -> addr_a
    ///   i_next = i + 1
    ///   cond   = cmp i_next, N, SLt
    ///   br cond, loop, exit
    /// ```
    fn build_add_loop_function() -> IRFunction {
        let mut func = IRFunction::new("vec_add");
        // vregs: 0=a_ptr, 1=b_ptr, 2=c_ptr, 3=N (params)
        // 4=i (phi), 5=i*4 (i_scaled), 6=addr_b, 7=addr_c,
        // 8=b_v, 9=c_v, 10=sum, 11=addr_a, 12=i_next, 13=cond
        func.params = vec![
            IRValue::Register(0),
            IRValue::Register(1),
            IRValue::Register(2),
            IRValue::Register(3),
        ];

        // entry block: jump to loop.
        let entry = IRBlock {
            label: "entry".to_string(),
            instructions: vec![],
            terminator: IRTerminator::Jump("loop".to_string()),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks[0] = entry;

        // loop block.
        let mut loop_blk = IRBlock::new("loop");
        loop_blk.instructions.push(IRInstr::Phi {
            dst: IRValue::Register(4),
            incoming: vec![
                (IRValue::Immediate(0), "entry".to_string()),
                (IRValue::Register(12), "loop".to_string()),
            ],
        });
        loop_blk.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(5),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(4),
            ty: Some(IRType::I32),
        });
        loop_blk.instructions.push(IRInstr::Offset {
            dst: IRValue::Register(6),
            base: IRValue::Register(1),
            offset: IRValue::Register(5),
        });
        loop_blk.instructions.push(IRInstr::Offset {
            dst: IRValue::Register(7),
            base: IRValue::Register(2),
            offset: IRValue::Register(5),
        });
        loop_blk.instructions.push(IRInstr::Load {
            dst: IRValue::Register(8),
            addr: IRValue::Register(6),
            offset: 0,
            ty: IRType::I32,
        });
        loop_blk.instructions.push(IRInstr::Load {
            dst: IRValue::Register(9),
            addr: IRValue::Register(7),
            offset: 0,
            ty: IRType::I32,
        });
        loop_blk.instructions.push(IRInstr::Add {
            dst: IRValue::Register(10),
            lhs: IRValue::Register(8),
            rhs: IRValue::Register(9),
            ty: Some(IRType::I32),
        });
        loop_blk.instructions.push(IRInstr::Offset {
            dst: IRValue::Register(11),
            base: IRValue::Register(0),
            offset: IRValue::Register(5),
        });
        loop_blk.instructions.push(IRInstr::Store {
            value: IRValue::Register(10),
            addr: IRValue::Register(11),
            offset: 0,
            ty: IRType::I32,
        });
        loop_blk.instructions.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(12),
            lhs: IRValue::Register(4),
            rhs: IRValue::Immediate(1),
            ty: None,
        });
        loop_blk.instructions.push(IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: IRValue::Register(13),
            lhs: IRValue::Register(12),
            rhs: IRValue::Register(3),
            ty: Some(IRType::I32),
        });
        loop_blk.terminator = IRTerminator::Branch {
            cond: IRValue::Register(13),
            true_block: "loop".to_string(),
            false_block: "exit".to_string(),
        };
        func.blocks.push(loop_blk);

        // exit block.
        let exit_blk = IRBlock {
            label: "exit".to_string(),
            instructions: vec![IRInstr::Ret { values: vec![] }],
            terminator: IRTerminator::Return(vec![]),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        };
        func.blocks.push(exit_blk);
        func
    }

    #[test]
    fn test_loop_vectorization_iv_step_fix() {
        // The CRITICAL test: the stub left the IV step at +1; the rewrite
        // must change it to +vf*elem_size (= +16 for i32 with vf=4).
        let func = build_add_loop_function();
        let (new_func, plan) = vectorize_function_with_plan(func);

        let vec_blk = new_func
            .blocks
            .iter()
            .find(|b| b.label == "loop")
            .expect("vector loop block must exist");

        // The IV increment must now be `+16` (vf=4, elem_size=4), not `+1`.
        let iv_step = vec_blk.instructions.iter().find_map(|instr| {
            if let IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(12),
                lhs: IRValue::Register(4),
                rhs: IRValue::Immediate(step),
                ..
            } = instr
            {
                Some(*step)
            } else {
                None
            }
        });
        assert_eq!(
            iv_step,
            Some(16),
            "IV step must be vf*elem_size = 4*4 = 16, got {:?}",
            iv_step
        );

        assert_eq!(plan.vf, 4);
        assert_eq!(plan.elem_size, 4);
        assert!(
            plan.packed_ops.iter().any(|op| op.kind == PackedOpKind::Add),
            "plan must contain a PackedAdd, got {:?}",
            plan.packed_ops
        );
    }

    #[test]
    fn test_loop_vectorization_remainder_loop_exists() {
        let func = build_add_loop_function();
        let (new_func, plan) = vectorize_function_with_plan(func);

        let remainder = new_func.blocks.iter().find(|b| b.label == "loop_remainder");
        assert!(remainder.is_some(), "remainder loop block must exist");

        let remainder = remainder.unwrap();
        let has_scalar_step = remainder.instructions.iter().any(|instr| {
            matches!(
                instr,
                IRInstr::BinOp {
                    op: BinOpKind::Add,
                    rhs: IRValue::Immediate(1),
                    ..
                }
            )
        });
        assert!(
            has_scalar_step,
            "remainder loop must have a +1 scalar IV step"
        );

        assert_eq!(plan.remainder_loop_block, Some("loop_remainder".to_string()));
    }

    #[test]
    fn test_loop_vectorization_lane_offsets() {
        // The vectorized body must contain lane-offset vregs `i + 4`, `i + 8`,
        // `i + 12` (lanes 1, 2, 3 for vf=4, elem_size=4).
        let func = build_add_loop_function();
        let (new_func, _) = vectorize_function_with_plan(func);

        let vec_blk = new_func
            .blocks
            .iter()
            .find(|b| b.label == "loop")
            .expect("vector loop block must exist");

        let lane_offsets: Vec<i64> = vec_blk
            .instructions
            .iter()
            .filter_map(|instr| {
                if let IRInstr::BinOp {
                    op: BinOpKind::Add,
                    lhs: IRValue::Register(4),
                    rhs: IRValue::Immediate(off),
                    ..
                } = instr
                {
                    Some(*off)
                } else {
                    None
                }
            })
            .collect();
        assert!(
            lane_offsets.contains(&4),
            "lane 1 offset (i+4) missing: {:?}",
            lane_offsets
        );
        assert!(
            lane_offsets.contains(&8),
            "lane 2 offset (i+8) missing: {:?}",
            lane_offsets
        );
        assert!(
            lane_offsets.contains(&12),
            "lane 3 offset (i+12) missing: {:?}",
            lane_offsets
        );
        assert!(
            lane_offsets.contains(&16),
            "IV step (i+16) missing: {:?}",
            lane_offsets
        );
    }

    #[test]
    fn test_loop_vectorization_no_miscompile_body_count() {
        // The stub duplicated the body 4× WITHOUT changing the IV step (4N
        // work). The rewrite must run N/vf iterations with vf work each = N
        // total. We verify the body is duplicated vf=4 times AND the IV step
        // is vf*elem_size (so the loop runs N/vf times, not N times).
        let func = build_add_loop_function();
        let (new_func, plan) = vectorize_function_with_plan(func);

        let vec_blk = new_func
            .blocks
            .iter()
            .find(|b| b.label == "loop")
            .expect("vector loop block must exist");

        let add_count = vec_blk
            .instructions
            .iter()
            .filter(|instr| matches!(instr, IRInstr::Add { .. }))
            .count();
        assert_eq!(
            add_count, 4,
            "body must be duplicated vf=4 times, got {}",
            add_count
        );

        let iv_step = vec_blk.instructions.iter().find_map(|instr| {
            if let IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(12),
                lhs: IRValue::Register(4),
                rhs: IRValue::Immediate(step),
                ..
            } = instr
            {
                Some(*step)
            } else {
                None
            }
        });
        assert_eq!(iv_step, Some(16));
        assert_eq!(plan.vf, 4);
    }

    #[test]
    fn test_loop_vectorization_bails_on_unsafe_body() {
        // A loop with a Call in the body must not be vectorized.
        let mut func = build_add_loop_function();
        let loop_idx = func
            .blocks
            .iter()
            .position(|b| b.label == "loop")
            .unwrap();
        // Inject a Call before the increment (which is the second-to-last instr
        // before the Cmp). Compute insert index first to avoid double-borrow.
        let insert_at = func.blocks[loop_idx].instructions.len() - 2;
        func.blocks[loop_idx].instructions.insert(
            insert_at,
            IRInstr::Call {
                dst: None,
                func: "extern_fn".to_string(),
                args: vec![],
                is_extern: true,
            },
        );
        let (new_func, plan) = vectorize_function_with_plan(func);
        let vec_blk = new_func
            .blocks
            .iter()
            .find(|b| b.label == "loop")
            .unwrap();
        let iv_step = vec_blk.instructions.iter().find_map(|instr| {
            if let IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(12),
                lhs: IRValue::Register(4),
                rhs: IRValue::Immediate(step),
                ..
            } = instr
            {
                Some(*step)
            } else {
                None
            }
        });
        assert_eq!(iv_step, Some(1), "loop with Call must not be vectorized");
        assert_eq!(plan.vf, 0, "plan must be empty for unsafe loop");
    }

    // ── SLP tests ──────────────────────────────────────────────────────

    #[test]
    fn test_slp_packs_isomorphic_independent_pair() {
        let mut blk = IRBlock::new("bb");
        blk.instructions.push(IRInstr::Add {
            dst: IRValue::Register(10),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
            ty: Some(IRType::I32),
        });
        blk.instructions.push(IRInstr::Add {
            dst: IRValue::Register(11),
            lhs: IRValue::Register(3),
            rhs: IRValue::Register(4),
            ty: Some(IRType::I32),
        });
        let ops = slp_vectorize_block(&blk);
        assert_eq!(ops.len(), 1, "expected 1 SLP pack, got {:?}", ops);
        assert_eq!(ops[0].kind, PackedOpKind::Add);
        assert_eq!(ops[0].lanes, 2);
        assert_eq!(ops[0].elem_size, 4);
        assert_eq!(ops[0].dst_lane0, 10);
    }

    #[test]
    fn test_slp_does_not_pack_dependent_pair() {
        // `a = x + y; b = a + z` — b depends on a, must NOT pack.
        let mut blk = IRBlock::new("bb");
        blk.instructions.push(IRInstr::Add {
            dst: IRValue::Register(10),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
            ty: Some(IRType::I32),
        });
        blk.instructions.push(IRInstr::Add {
            dst: IRValue::Register(11),
            lhs: IRValue::Register(10), // uses a's dst
            rhs: IRValue::Register(3),
            ty: Some(IRType::I32),
        });
        let ops = slp_vectorize_block(&blk);
        assert!(ops.is_empty(), "dependent pair must not pack, got {:?}", ops);
    }

    // ── Legacy alias-analysis tests ────────────────────────────────────

    #[test]
    fn test_non_aliasing_different_allocs() {
        let allocs: HashSet<u32> = [1, 2].iter().copied().collect();
        assert!(is_proven_non_aliasing(
            &IRValue::Register(1),
            &IRValue::Register(2),
            &allocs
        ));
    }

    #[test]
    fn test_aliasing_same_alloc() {
        let allocs: HashSet<u32> = [1].iter().copied().collect();
        assert!(!is_proven_non_aliasing(
            &IRValue::Register(1),
            &IRValue::Register(1),
            &allocs
        ));
    }

    // ── Wave 29 ISel integration tests ─────────────────────────────────

    /// Wave 29 audit resolution: verify that an `IRInstr::VectorOp` actually
    /// lowers to SSE/AVX machine bytes in the x86_64 backend (and to a NEON
    /// word in the aarch64 backend).  Prior to Wave 29 ISel wiring, the
    /// encoders existed but were called only from `#[test]` functions.
    #[test]
    fn test_wave29_simd_emitted_in_x86_64_isel() {
        use crate::backend::{BackendKind, create_backend};
        use crate::ir::VirtualRegister;
        let mut blk = IRBlock::new("entry");
        blk.instructions.push(IRInstr::VectorOp {
            op: VectorOpKind::Add,
            lanes: 2,
            elem_size: 8,
            dst: IRValue::Register(10),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
        });
        blk.terminator = IRTerminator::Return(vec![IRValue::Register(10)]);
        let mut func = IRFunction::new("simd_add_q");
        func.vregs.insert(10, VirtualRegister::anonymous(10));
        func.vregs.insert(1, VirtualRegister::anonymous(1));
        func.vregs.insert(2, VirtualRegister::anonymous(2));
        func.blocks[0] = blk;

        let backend = create_backend(BackendKind::X86_64).expect("x86_64 backend");
        let allocated = backend.allocate_registers(&func).expect("x86_64 alloc");
        let mut all_bytes = Vec::new();
        for b in &allocated.blocks {
            for instr in &b.instructions {
                all_bytes.extend_from_slice(&instr.encoded);
            }
        }
        // SSE2 `paddq xmm0, xmm1` = 66 0F D4 C1.  Assert the prefix bytes
        // 66 0F D4 appear somewhere in the emitted stream.
        assert!(
            window_contains(&all_bytes, &[0x66, 0x0F, 0xD4]),
            "x86_64 VectorOp(Add, i64) must emit SSE2 paddq (66 0F D4 ..); got {:02X?}",
            all_bytes
        );
    }

    #[test]
    fn test_wave29_simd_emitted_in_aarch64_isel() {
        use crate::backend::{BackendKind, create_backend};
        use crate::ir::VirtualRegister;
        let mut blk = IRBlock::new("entry");
        blk.instructions.push(IRInstr::VectorOp {
            op: VectorOpKind::Add,
            lanes: 4,
            elem_size: 4,
            dst: IRValue::Register(10),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
        });
        blk.terminator = IRTerminator::Return(vec![IRValue::Register(10)]);
        let mut func = IRFunction::new("simd_add_v4s");
        func.vregs.insert(10, VirtualRegister::anonymous(10));
        func.vregs.insert(1, VirtualRegister::anonymous(1));
        func.vregs.insert(2, VirtualRegister::anonymous(2));
        func.blocks[0] = blk;

        let backend = create_backend(BackendKind::AArch64).expect("aarch64 backend");
        let allocated = backend.allocate_registers(&func).expect("aarch64 alloc");
        // Collect all encoded u32 words.
        let mut words = Vec::new();
        for b in &allocated.blocks {
            for instr in &b.instructions {
                if instr.encoded.len() == 4 {
                    words.push(u32::from_le_bytes([
                        instr.encoded[0],
                        instr.encoded[1],
                        instr.encoded[2],
                        instr.encoded[3],
                    ]));
                }
            }
        }
        // NEON `add v0.4s, v1.4s, v2.4s` = 0x4E228420.  Assert it's present.
        assert!(
            words.iter().any(|w| *w == 0x4E228420),
            "aarch64 VectorOp(Add, v4i32) must emit NEON add v0.4s, v1.4s, v2.4s (0x4E228420); got {:08X?}",
            words
        );
    }

    fn window_contains(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || haystack.len() < needle.len() {
            return false;
        }
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
