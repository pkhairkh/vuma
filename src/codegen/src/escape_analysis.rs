//! Escape Analysis for Stack Allocation
//!
//! Determines which allocations don't escape their function and can be
//! stack-allocated instead of heap-allocated.
//!
//! # Algorithm
//!
//! 1. For each Alloc instruction (and each `__vuma_alloc`/`allocate` Call
//!    that produces a fresh pointer), track the resulting vreg.
//! 2. An allocation ESCAPES if:
//!    - It's returned from the function (Ret with the vreg)
//!    - It's stored to memory (Store with the vreg as value)
//!    - It's passed to a Call as an argument (except to free())
//!    - It's used in a Phi that could propagate to an escape
//! 3. Non-escaping allocations are marked for stack allocation.
//!
//! # Wave 32 Additions
//!
//! Wave 32 wires this analysis into the O2+ pipeline (see `pipeline.rs`)
//! and adds two optimisation transforms driven by the escape info:
//!
//! * [`scalar_replace_aggregates`] (SROA): for each non-escaping
//!   allocation whose accesses are all direct (constant-offset
//!   Load/Store through the alloc's pointer, no Offset/Phi
//!   indirection), replace the alloc+field-accesses with individual
//!   scalar virtual registers.  This eliminates the alloc and exposes
//!   the field values to constant folding, CSE, etc.
//!
//! * [`elide_non_escaping_allocs`]: for each non-escaping heap
//!   allocation (`__vuma_alloc`/`allocate` Call) whose memory is
//!   never read or written, drop the alloc call and its matching
//!   `__vuma_free`/`free`/`Free` instruction.  This is the
//!   "allocation elision" optimisation.

use std::collections::{HashMap, HashSet};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator};

/// Result of escape analysis for a single allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeResult {
    /// Allocation does not escape — can be stack-allocated.
    DoesNotEscape,
    /// Allocation escapes — must be heap-allocated.
    Escapes,
}

/// Returns `true` if `fname` is a heap-allocation runtime call.
fn is_alloc_call(fname: &str) -> bool {
    matches!(fname, "__vuma_alloc" | "allocate")
}

/// Returns `true` if `fname` is a heap-deallocation runtime call.
fn is_free_call(fname: &str) -> bool {
    matches!(fname, "__vuma_free" | "free")
}

/// Returns `true` if the use of allocation vreg `vreg` in `instr` is a
/// "safe" direct access that does NOT cause the allocation to escape and
/// can be handled by SROA (i.e., a direct `Load`/`Store`/`AtomicLoad`/
/// `AtomicStore`/`AtomicCas` whose `addr` is exactly `vreg`), or is the
/// matching deallocator (`Free` / `__vuma_free` / `free` call with
/// `vreg` as its sole argument).
///
/// Any OTHER use of `vreg` — pointer arithmetic (`Add`/`Offset`),
/// type casts (`Cast`), control-merge (`Phi`/`Select`), passing to a
/// non-`free` call, storing as a value, returning, etc. — is treated as
/// an escape by [`analyze_escapes`].
fn is_safe_alloc_use(instr: &IRInstr, vreg: u32) -> bool {
    match instr {
        IRInstr::Load { addr, .. } | IRInstr::Store { addr, .. }
            if addr.as_register() == Some(vreg) => true,
        IRInstr::AtomicLoad { addr, .. } | IRInstr::AtomicStore { addr, .. }
            if addr.as_register() == Some(vreg) => true,
        IRInstr::AtomicCas { addr, .. } if addr.as_register() == Some(vreg) => true,
        IRInstr::Free { ptr } if ptr.as_register() == Some(vreg) => true,
        IRInstr::Call { func: fname, args, .. }
            if is_free_call(fname)
                && args.len() == 1
                && args[0].as_register() == Some(vreg) =>
        {
            true
        }
        _ => false,
    }
}

/// Collect all vreg uses from a terminator (for escape-point detection).
fn terminator_used_regs(term: &IRTerminator) -> Vec<u32> {
    match term {
        IRTerminator::Return(vals) => vals.iter().filter_map(|v| v.as_register()).collect(),
        IRTerminator::Branch { cond, .. } => cond.as_register().into_iter().collect(),
        IRTerminator::Switch { discr, .. } => discr.as_register().into_iter().collect(),
        IRTerminator::Invoke { args, .. } => {
            args.iter().filter_map(|v| v.as_register()).collect()
        }
        IRTerminator::TailCall { args, .. } => {
            args.iter().filter_map(|v| v.as_register()).collect()
        }
        IRTerminator::Resume { value } => value.as_register().into_iter().collect(),
        _ => vec![],
    }
}

/// Analyze a function for escaping allocations.
///
/// Returns a map from vreg (allocation result) to escape result.
/// Both `IRInstr::Alloc` (stack) and `IRInstr::Call` to
/// `__vuma_alloc`/`allocate` (heap) are tracked as allocations.
///
/// **Soundness (Wave 3 fix):** An allocation ESCAPES unless PROVEN
/// non-escaping. The only "safe" (non-escaping) uses of an allocation
/// vreg are:
///   * direct `Load`/`Store`/`AtomicLoad`/`AtomicStore`/`AtomicCas`
///     whose `addr` operand is exactly the alloc vreg (SROA candidates),
///   * the matching `Free` / `__vuma_free` / `free` call.
/// ANY other use — pointer arithmetic (`Add`/`Sub`/`Offset`), type
/// casts (`Cast`), control-merge (`Phi`/`Select`), storing the alloc
/// address as a value, passing to a non-`free` call, passing to a
/// syscall, returning, branching on it, etc. — marks the allocation as
/// escaping. This prevents SROA / alloc-elision from removing an
/// allocation whose address is observed through a derived alias (the
/// root cause of the Wave 2 SIGSEGV regressions on `mem_copy_buffer`,
/// `doubly_linked_list`, `mf_address_return`, etc.).
pub fn analyze_escapes(func: &IRFunction) -> HashMap<u32, EscapeResult> {
    let mut allocs: HashSet<u32> = HashSet::new();
    let mut escapes: HashSet<u32> = HashSet::new();

    // Phase 1: Find all allocations (stack Alloc + heap alloc calls +
    // direct mmap syscalls from P2's allocate() lowering).
    for block in &func.blocks {
        for instr in &block.instructions {
            match instr {
                IRInstr::Alloc { dst, .. } => {
                    if let Some(vreg) = dst.as_register() {
                        allocs.insert(vreg);
                    }
                }
                IRInstr::Call { dst: Some(dst), func: fname, .. }
                    if is_alloc_call(fname) =>
                {
                    if let Some(vreg) = dst.as_register() {
                        allocs.insert(vreg);
                    }
                }
                // P2: allocate() lowers to syscall(222, mmap). The result
                // vreg is a heap allocation that must be tracked for escape
                // analysis (otherwise the buffer passed to write() wouldn't
                // be marked as escaping, and SROA/elision could remove it).
                IRInstr::Syscall { nr: 222, dst: Some(dst), .. } => {
                    if let Some(vreg) = dst.as_register() {
                        allocs.insert(vreg);
                    }
                }
                _ => {}
            }
        }
    }

    // Phase 2: Find escape points.
    //
    // For each instruction, for each vreg it uses that is an allocation,
    // check whether the use is "safe" (direct memory access or matching
    // free). If not, the allocation escapes. This catches:
    //   * `Store` of the alloc address as a value → escape
    //   * `Call` arg (non-`free`) → escape
    //   * `Syscall` arg → escape
    //   * `Add`/`Sub`/`Offset`/`Cast`/`Select`/`Phi` of the alloc → escape
    //     (the derived alias may be stored/returned/passed later, and we
    //      cannot track aliases soundly without a full alias analysis)
    //   * `Return`/`Branch`/`Switch`/`Invoke`/`TailCall`/`Resume` → escape
    for block in &func.blocks {
        for instr in &block.instructions {
            // The Alloc instruction itself defines the vreg; skip it.
            if matches!(instr, IRInstr::Alloc { .. }) {
                continue;
            }
            for vreg in instr.used_regs() {
                if allocs.contains(&vreg) && !is_safe_alloc_use(instr, vreg) {
                    escapes.insert(vreg);
                }
            }
        }

        // Check terminator vreg uses (Return, Branch cond, Switch discr,
        // Invoke args, TailCall args, Resume value).
        for vreg in terminator_used_regs(&block.terminator) {
            if allocs.contains(&vreg) {
                escapes.insert(vreg);
            }
        }
    }

    // Phase 3: Build result map
    let mut result = HashMap::new();
    for &alloc_vreg in &allocs {
        let escape = if escapes.contains(&alloc_vreg) {
            EscapeResult::Escapes
        } else {
            EscapeResult::DoesNotEscape
        };
        result.insert(alloc_vreg, escape);
    }

    result
}

/// Program-wide escape analysis.
///
/// Returns a map from function name to that function's per-alloc escape info.
/// Used by the O2+ pipeline to drive SROA and alloc elision across the
/// whole program without re-running the analysis per pass.
pub fn analyze_escapes_program(funcs: &[IRFunction]) -> HashMap<String, HashMap<u32, EscapeResult>> {
    funcs
        .iter()
        .map(|f| (f.name.clone(), analyze_escapes(f)))
        .collect()
}

/// Count how many allocations can be stack-allocated.
pub fn count_stack_allocatable(func: &IRFunction) -> (usize, usize) {
    let results = analyze_escapes(func);
    let total = results.len();
    let stack = results
        .values()
        .filter(|r| **r == EscapeResult::DoesNotEscape)
        .count();
    (stack, total)
}

// ── Helpers for SROA / alloc elision ──────────────────────────────────────

/// Reserve a stable "field" vreg ID for `(alloc_vreg, offset)`.
///
/// Uses the high bit pattern `0x4000_0000 + alloc_vreg*0x10000 + offset`
/// to produce IDs that don't collide with normal vregs (which are
/// typically small sequential integers).  Collisions between two
/// different (alloc, offset) pairs are possible in theory but extremely
/// unlikely in practice (would require >64K allocs in one function).
fn field_vreg(alloc_vreg: u32, offset: i32) -> u32 {
    const FIELD_BASE: u32 = 0x4000_0000;
    let off = (offset as u32) & 0xFFFF;
    FIELD_BASE
        .wrapping_add(alloc_vreg.wrapping_mul(0x10000))
        .wrapping_add(off)
}

/// Rename every occurrence of `from` -> `to` in `func` (every `IRValue`
/// position — both definitions and uses, in both instructions and
/// terminators).  Used by SROA to rewrite Load/Store operand vregs to
/// per-field scalars.
fn rename_vreg_everywhere(func: &mut IRFunction, from: u32, to: u32) {
    fn sub(v: &mut IRValue, from: u32, to: u32) {
        if let IRValue::Register(r) = v {
            if *r == from {
                *r = to;
            }
        }
    }
    for block in &mut func.blocks {
        for instr in &mut block.instructions {
            match instr {
                IRInstr::Load { dst, addr, .. } => {
                    sub(dst, from, to);
                    sub(addr, from, to);
                }
                IRInstr::Store { value, addr, .. } => {
                    sub(value, from, to);
                    sub(addr, from, to);
                }
                IRInstr::BinOp { dst, lhs, rhs, .. } => {
                    sub(dst, from, to);
                    sub(lhs, from, to);
                    sub(rhs, from, to);
                }
                IRInstr::Add { dst, lhs, rhs, .. }
                | IRInstr::Sub { dst, lhs, rhs, .. }
                | IRInstr::Mul { dst, lhs, rhs, .. }
                | IRInstr::Div { dst, lhs, rhs, .. } => {
                    sub(dst, from, to);
                    sub(lhs, from, to);
                    sub(rhs, from, to);
                }
                IRInstr::Cmp { dst, lhs, rhs, .. } => {
                    sub(dst, from, to);
                    sub(lhs, from, to);
                    sub(rhs, from, to);
                }
                IRInstr::UnaryOp { dst, operand, .. } => {
                    sub(dst, from, to);
                    sub(operand, from, to);
                }
                IRInstr::Call { dst, args, .. } => {
                    if let Some(d) = dst {
                        sub(d, from, to);
                    }
                    for a in args {
                        sub(a, from, to);
                    }
                }
                IRInstr::Alloc { dst, .. } => sub(dst, from, to),
                IRInstr::Free { ptr } => sub(ptr, from, to),
                IRInstr::Cast { dst, src, .. } => {
                    sub(dst, from, to);
                    sub(src, from, to);
                }
                IRInstr::Phi { dst, incoming } => {
                    sub(dst, from, to);
                    for (v, _) in incoming {
                        sub(v, from, to);
                    }
                }
                IRInstr::GetAddress { dst, .. } => sub(dst, from, to),
                IRInstr::Offset { dst, base, offset } => {
                    sub(dst, from, to);
                    sub(base, from, to);
                    sub(offset, from, to);
                }
                IRInstr::Select {
                    dst,
                    cond,
                    true_val,
                    false_val,
                    ..
                } => {
                    sub(dst, from, to);
                    sub(cond, from, to);
                    sub(true_val, from, to);
                    sub(false_val, from, to);
                }
                IRInstr::AtomicLoad { dst, addr, .. } => {
                    sub(dst, from, to);
                    sub(addr, from, to);
                }
                IRInstr::AtomicStore { value, addr, .. } => {
                    sub(value, from, to);
                    sub(addr, from, to);
                }
                IRInstr::AtomicCas {
                    dst,
                    addr,
                    expected,
                    desired,
                    ..
                } => {
                    sub(dst, from, to);
                    sub(addr, from, to);
                    sub(expected, from, to);
                    sub(desired, from, to);
                }
                IRInstr::Syscall { dst, args, .. } => {
                    if let Some(d) = dst {
                        sub(d, from, to);
                    }
                    for a in args {
                        sub(a, from, to);
                    }
                }
                IRInstr::Ret { values } => {
                    for v in values {
                        sub(v, from, to);
                    }
                }
                IRInstr::CondBranch { cond, .. } => sub(cond, from, to),
                IRInstr::CtSelect {
                    dst,
                    cond,
                    true_val,
                    false_val,
                    ..
                } => {
                    sub(dst, from, to);
                    sub(cond, from, to);
                    sub(true_val, from, to);
                    sub(false_val, from, to);
                }
                IRInstr::CtEq { dst, lhs, rhs, .. } => {
                    sub(dst, from, to);
                    sub(lhs, from, to);
                    sub(rhs, from, to);
                }
                IRInstr::Branch { .. } => {}
                // ── VectorOp (Wave 29) ──
                // Substitute dst/lhs/rhs vregs (lanes/elem_size are not vregs).
                IRInstr::VectorOp { dst, lhs, rhs, .. } => {
                    sub(dst, from, to);
                    sub(lhs, from, to);
                    sub(rhs, from, to);
                }
                // ── Channel operations (Wave 1d / Task 2a) ──
                // Vreg renumbering applies to all operands (including opaque
                // channel handles, which are ordinary vregs at the IR level).
                IRInstr::ChannelOpen { dst, .. } => sub(dst, from, to),
                IRInstr::ChannelSend { ch, msg, .. } => {
                    sub(ch, from, to);
                    sub(msg, from, to);
                }
                IRInstr::ChannelRecv { ch, dst, .. } => {
                    sub(ch, from, to);
                    sub(dst, from, to);
                }
                IRInstr::ChannelRecvTimeout { ch, dst, .. } => {
                    sub(ch, from, to);
                    sub(dst, from, to);
                }
                // Wave 8b: renumber ch, value dst, and err_dst.
                IRInstr::ChannelRecvResult { ch, dst, err_dst, .. } => {
                    sub(ch, from, to);
                    sub(dst, from, to);
                    sub(err_dst, from, to);
                }
                IRInstr::ChannelClose { ch } => sub(ch, from, to),
            }
        }
        match &mut block.terminator {
            IRTerminator::Return(vals) => {
                for v in vals {
                    sub(v, from, to);
                }
            }
            IRTerminator::Branch { cond, .. } => sub(cond, from, to),
            IRTerminator::Switch { discr, .. } => sub(discr, from, to),
            IRTerminator::Invoke { dst, args, .. } => {
                if let Some(d) = dst {
                    sub(d, from, to);
                }
                for a in args {
                    sub(a, from, to);
                }
            }
            IRTerminator::TailCall { args, .. } => {
                for a in args {
                    sub(a, from, to);
                }
            }
            IRTerminator::Resume { value } => sub(value, from, to),
            _ => {}
        }
    }
}

/// Description of a single direct access to an allocation.
struct Access {
    block_idx: usize,
    instr_idx: usize,
    offset: i32,
    /// `true` for Store, `false` for Load.
    is_store: bool,
    /// The vreg being read (Load.dst) or written (Store.value).
    value_vreg: u32,
}

/// Scalar Replacement of Aggregates (SROA).
///
/// For each non-escaping allocation in `func` (per `escape_info`) whose
/// accesses are all direct constant-offset Load/Store through the
/// allocation's pointer, replace the alloc+accesses with individual
/// scalar virtual registers — one per `(offset, ty)` pair.
///
/// Returns the number of allocations promoted.  The transform is
/// conservative: it bails out on any allocation whose address is
/// derived through an `Offset` instruction or propagated through a
/// `Phi`, and on any `(offset, ty)` group with more than one Store
/// (which would require SSA construction to merge).
pub fn scalar_replace_aggregates(
    func: &mut IRFunction,
    escape_info: &HashMap<u32, EscapeResult>,
) -> usize {
    let non_escaping: Vec<u32> = escape_info
        .iter()
        .filter(|(_, r)| **r == EscapeResult::DoesNotEscape)
        .map(|(v, _)| *v)
        .collect();

    let mut promoted = 0usize;
    for alloc_vreg in non_escaping {
        // Phase A: collect direct accesses; bail on indirection.
        let mut accesses: Vec<Access> = Vec::new();
        let mut has_indirection = false;
        'outer: for (bi, block) in func.blocks.iter().enumerate() {
            for (ii, instr) in block.instructions.iter().enumerate() {
                match instr {
                    IRInstr::Load { dst, addr, offset, .. } => {
                        if let (Some(dst_v), Some(addr_v)) = (dst.as_register(), addr.as_register())
                        {
                            if addr_v == alloc_vreg {
                                accesses.push(Access {
                                    block_idx: bi,
                                    instr_idx: ii,
                                    offset: *offset,
                                    is_store: false,
                                    value_vreg: dst_v,
                                });
                                continue;
                            }
                        }
                    }
                    IRInstr::Store { value, addr, offset, .. } => {
                        if let Some(addr_v) = addr.as_register() {
                            if addr_v == alloc_vreg {
                                // SROA's rename framework can only handle
                                // register-typed store values (it renames
                                // the value vreg to the field vreg). An
                                // Immediate/Address store value cannot be
                                // renamed — SROA would need to materialize
                                // the immediate into the field vreg, which
                                // the current implementation does not do.
                                // BAIL on any alloc with an immediate store
                                // to preserve correctness (the alloc keeps
                                // its memory representation).
                                //
                                // Without this bail, SROA would skip the
                                // immediate store in `accesses`, under-count
                                // stores_per_offset, and promote the alloc
                                // while the immediate store remains — the
                                // promoted field vreg never receives the
                                // immediate value, loads read undef → SIGSEGV.
                                // (This was the conc_swap/ptr_swap crash
                                // cluster: `*ptr = 2; ... *ptr = reg + 0`
                                // has an immediate store SROA missed.)
                                if value.as_register().is_none() {
                                    has_indirection = true;
                                    break 'outer;
                                }
                                let val_v = value.as_register().unwrap();
                                accesses.push(Access {
                                    block_idx: bi,
                                    instr_idx: ii,
                                    offset: *offset,
                                    is_store: true,
                                    value_vreg: val_v,
                                });
                                continue;
                            }
                        }
                    }
                    IRInstr::Offset { base, .. } => {
                        if let Some(b) = base.as_register() {
                            if b == alloc_vreg {
                                has_indirection = true;
                                break 'outer;
                            }
                        }
                    }
                    IRInstr::Phi { incoming, .. } => {
                        for (v, _) in incoming {
                            if let Some(r) = v.as_register() {
                                if r == alloc_vreg {
                                    has_indirection = true;
                                    break 'outer;
                                }
                            }
                        }
                    }
                    // Defense-in-depth (Wave 3): the Alloc itself and the
                    // matching Free/__vuma_free call are allowed (they don't
                    // create aliases). Any OTHER instruction that uses the
                    // alloc vreg — `Add`/`Sub`/`Cast`/`Select`/`Call`/
                    // `Syscall`/etc. — means the alloc address is observed
                    // through a derived alias or escape path that SROA
                    // cannot safely promote. Bail.
                    _ => {
                        // Skip the Alloc instruction (it defines, not uses).
                        if matches!(instr, IRInstr::Alloc { .. }) {
                            continue;
                        }
                        // Skip the matching Free/__vuma_free call.
                        let is_matching_free = matches!(instr,
                            IRInstr::Free { ptr } if ptr.as_register() == Some(alloc_vreg))
                            || matches!(instr,
                            IRInstr::Call { func: fname, args, .. }
                            if is_free_call(fname) && args.len() == 1
                                && args[0].as_register() == Some(alloc_vreg));
                        if is_matching_free {
                            continue;
                        }
                        if instr.used_regs().contains(&alloc_vreg) {
                            has_indirection = true;
                            break 'outer;
                        }
                    }
                }
            }
        }
        if has_indirection || accesses.is_empty() {
            continue;
        }

        // Phase B: group by offset; ensure at most 1 Store per offset
        // (more would require SSA construction, which we defer).
        let mut stores_per_offset: HashMap<i32, u32> = HashMap::new();
        let mut loads_per_offset: HashMap<i32, u32> = HashMap::new();
        for a in &accesses {
            if a.is_store {
                *stores_per_offset.entry(a.offset).or_insert(0) += 1;
            } else {
                *loads_per_offset.entry(a.offset).or_insert(0) += 1;
            }
        }
        // Bail if any offset has >1 Store.
        if stores_per_offset.values().any(|c| *c > 1) {
            continue;
        }
        // Bail if any offset has Loads but no Store (would read undef).
        if loads_per_offset.keys().any(|o| !stores_per_offset.contains_key(o)) {
            continue;
        }

        // Phase C: assign a field vreg per offset and rename.
        let mut field_map: HashMap<i32, u32> = HashMap::new();
        for a in &accesses {
            field_map
                .entry(a.offset)
                .or_insert_with(|| field_vreg(alloc_vreg, a.offset));
        }
        for a in &accesses {
            let field = *field_map.get(&a.offset).unwrap();
            if a.value_vreg != field {
                rename_vreg_everywhere(func, a.value_vreg, field);
            }
        }

        // Phase D: remove the Load/Store instructions (now dead).
        let mut to_remove: Vec<(usize, usize)> =
            accesses.iter().map(|a| (a.block_idx, a.instr_idx)).collect();
        to_remove.sort_unstable_by(|a, b| b.cmp(a));
        for (bi, ii) in to_remove {
            func.blocks[bi].instructions.remove(ii);
        }

        // Phase E: remove the Alloc / __vuma_alloc call itself.
        for block in &mut func.blocks {
            block.instructions.retain(|instr| match instr {
                IRInstr::Alloc { dst, .. } => dst.as_register() != Some(alloc_vreg),
                IRInstr::Call {
                    dst: Some(dst),
                    func: fname,
                    ..
                } if is_alloc_call(fname) => dst.as_register() != Some(alloc_vreg),
                _ => true,
            });
        }

        promoted += 1;
    }
    promoted
}

/// Elide `__vuma_alloc`/`__vuma_free` (and `Alloc`/`Free`) pairs for
/// non-escaping allocations whose memory is never read or written.
///
/// For each non-escaping allocation vreg `A`:
/// - If `A`'s vreg is used in ANY way other than the `Alloc`/`__vuma_alloc`
///   definition and the matching `Free`/`__vuma_free`/`free` call, skip
///   (the memory IS used — directly via `Load`/`Store`, or indirectly via
///   `Add`/`Offset`/`Cast` aliases that SROA bailed on).
/// - Otherwise (truly dead allocation), remove the producing `Alloc` or
///   `__vuma_alloc` Call, and remove the matching `Free` / `__vuma_free`
///   / `free` Call that takes `A` as its sole argument.
///
/// **Soundness (Wave 3 fix):** The previous check only looked for direct
/// `Load`/`Store` with `addr == A`. This missed accesses through derived
/// aliases (`A + i` computed via `Add`, then used as a `Store` addr),
/// causing the allocation to be incorrectly elided while the aliasing
/// `Add`/`Store` instructions still referenced the (now-removed) alloc
/// vreg — leading to SIGSEGV. The fix uses `IRInstr::used_regs()` to
/// detect ANY use of `A` outside its own definition and matching free.
///
/// Returns the number of allocation pairs elided.
pub fn elide_non_escaping_allocs(
    func: &mut IRFunction,
    escape_info: &HashMap<u32, EscapeResult>,
) -> usize {
    let non_escaping: Vec<u32> = escape_info
        .iter()
        .filter(|(_, r)| **r == EscapeResult::DoesNotEscape)
        .map(|(v, _)| *v)
        .collect();

    let mut elided = 0usize;
    for alloc_vreg in non_escaping {
        // SOUNDNESS: bail if the alloc vreg has ANY use other than the
        // Alloc/`__vuma_alloc` definition and the matching
        // Free/`__vuma_free`/`free` call. This catches:
        //   * direct Load/Store/AtomicLoad/AtomicStore/AtomicCas (memory
        //     IS accessed — SROA should have handled it, but bail if it
        //     didn't),
        //   * indirect accesses via Add/Offset/Cast/Select/Phi aliases
        //     (SROA bails on these; the alloc must be preserved),
        //   * any other use (Call arg, Syscall arg, Ret, etc.).
        let mut has_use = false;
        'use_check: for block in &func.blocks {
            for instr in &block.instructions {
                // Skip the Alloc / __vuma_alloc definition itself.
                let is_self_alloc = matches!(instr,
                    IRInstr::Alloc { dst, .. } if dst.as_register() == Some(alloc_vreg))
                    || matches!(instr,
                    IRInstr::Call { dst: Some(dst), func: fname, .. }
                    if is_alloc_call(fname) && dst.as_register() == Some(alloc_vreg));
                if is_self_alloc {
                    continue;
                }
                // Skip the matching Free / __vuma_free / free call.
                let is_matching_free = matches!(instr,
                    IRInstr::Free { ptr } if ptr.as_register() == Some(alloc_vreg))
                    || matches!(instr,
                    IRInstr::Call { func: fname, args, .. }
                    if is_free_call(fname) && args.len() == 1
                        && args[0].as_register() == Some(alloc_vreg));
                if is_matching_free {
                    continue;
                }
                if instr.used_regs().contains(&alloc_vreg) {
                    has_use = true;
                    break 'use_check;
                }
            }
        }
        // Also check terminators (Return, Branch cond, Switch discr,
        // Invoke args, TailCall args, Resume value).
        if !has_use {
            for block in &func.blocks {
                if terminator_used_regs(&block.terminator).contains(&alloc_vreg) {
                    has_use = true;
                    break;
                }
            }
        }
        if has_use {
            continue;
        }

        // Remove the Alloc / __vuma_alloc Call.
        let mut removed_alloc = false;
        for block in &mut func.blocks {
            block.instructions.retain(|instr| match instr {
                IRInstr::Alloc { dst, .. }
                    if dst.as_register() == Some(alloc_vreg) => {
                        removed_alloc = true;
                        false
                    }
                IRInstr::Call {
                    dst: Some(dst),
                    func: fname,
                    ..
                } if is_alloc_call(fname)
                    && dst.as_register() == Some(alloc_vreg) => {
                        removed_alloc = true;
                        false
                    }
                _ => true,
            });
        }
        if !removed_alloc {
            continue;
        }

        // Remove the matching Free / __vuma_free / free Call.
        // `retain` keeps instructions for which the predicate returns
        // `true`, so we return `true` to KEEP everything EXCEPT the
        // matching Free / __vuma_free call.
        for block in &mut func.blocks {
            block.instructions.retain(|instr| {
                let is_matching_free = match instr {
                    IRInstr::Free { ptr } => ptr.as_register() == Some(alloc_vreg),
                    IRInstr::Call {
                        func: fname,
                        args,
                        ..
                    } if is_free_call(fname) => {
                        args.len() == 1 && args[0].as_register() == Some(alloc_vreg)
                    }
                    _ => false,
                };
                !is_matching_free
            });
        }

        elided += 1;
    }
    elided
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IRBlock, IRType};

    /// Helper: make an `IRValue::Register(n)`.
    fn r(n: u32) -> IRValue {
        IRValue::Register(n)
    }

    /// Helper: build a function with one entry block whose instructions
    /// and terminator are provided.
    fn fn_with(name: &str, instrs: Vec<IRInstr>, term: IRTerminator) -> IRFunction {
        let mut f = IRFunction::new(name.to_string());
        f.blocks[0].instructions = instrs;
        f.blocks[0].terminator = term;
        f
    }

    #[test]
    fn test_empty_function() {
        let func = IRFunction::new("test".to_string());
        let (stack, total) = count_stack_allocatable(&func);
        assert_eq!(stack, 0);
        assert_eq!(total, 0);
    }

    // ── SROA tests ────────────────────────────────────────────────────

    /// Non-escaping `Alloc` with one Store and one Load at offset 0:
    /// SROA should remove the Alloc, Store, and Load, and rewrite the
    /// Load's dst vreg to match the Store's value vreg (so the consumer
    /// of the Load reads the stored value directly).
    #[test]
    fn test_sroa_promotes_non_escaping_alloc() {
        // %v1 = alloc 8
        // %v2 = add 0, 42      (some def of the value to store)
        // store %v2 -> (%v1, 0)
        // %v3 = load (%v1, 0)
        // %v4 = add %v3, 1
        // ret %v4
        let instrs = vec![
            IRInstr::Alloc {
                dst: r(1),
                size: 8,
            },
            IRInstr::Add {
                dst: r(2),
                lhs: IRValue::Immediate(0),
                rhs: IRValue::Immediate(42),
                ty: Some(IRType::I64),
            },
            IRInstr::Store {
                value: r(2),
                addr: r(1),
                offset: 0,
                ty: IRType::I64,
            },
            IRInstr::Load {
                dst: r(3),
                addr: r(1),
                offset: 0,
                ty: IRType::I64,
            },
            IRInstr::Add {
                dst: r(4),
                lhs: r(3),
                rhs: IRValue::Immediate(1),
                ty: Some(IRType::I64),
            },
        ];
        let mut func = fn_with("sroa_basic", instrs, IRTerminator::Return(vec![r(4)]));

        let info = analyze_escapes(&func);
        assert_eq!(
            info.get(&1).copied(),
            Some(EscapeResult::DoesNotEscape),
            "alloc should be non-escaping"
        );

        let promoted = scalar_replace_aggregates(&mut func, &info);
        assert_eq!(promoted, 1, "one alloc should be promoted");

        // No Alloc / Store / Load should remain.
        let allocs_remaining: usize = func
            .blocks
            .iter()
            .map(|b| {
                b.instructions
                    .iter()
                    .filter(|i| matches!(i, IRInstr::Alloc { .. }))
                    .count()
            })
            .sum();
        assert_eq!(allocs_remaining, 0, "Alloc should be removed");

        let loads_remaining: usize = func
            .blocks
            .iter()
            .map(|b| {
                b.instructions
                    .iter()
                    .filter(|i| matches!(i, IRInstr::Load { .. }))
                    .count()
            })
            .sum();
        assert_eq!(loads_remaining, 0, "Load should be removed");

        let stores_remaining: usize = func
            .blocks
            .iter()
            .map(|b| {
                b.instructions
                    .iter()
                    .filter(|i| matches!(i, IRInstr::Store { .. }))
                    .count()
            })
            .sum();
        assert_eq!(stores_remaining, 0, "Store should be removed");

        // The Add that defined %v2 should now define the field vreg,
        // and the consumer Add should read that field vreg.
        let field = field_vreg(1, 0);
        let consumer = func.blocks[0]
            .instructions
            .iter()
            .find_map(|i| match i {
                IRInstr::Add {
                    dst,
                    lhs,
                    rhs: IRValue::Immediate(1),
                    ..
                } => Some((dst.clone(), lhs.clone())),
                _ => None,
            })
            .expect("consumer Add should still exist");
        assert_eq!(
            consumer.1,
            IRValue::Register(field),
            "Load's dst should be renamed to the field vreg"
        );
        assert_ne!(
            consumer.0,
            IRValue::Register(field),
            "consumer's dst should not be the field vreg"
        );
    }

    /// Escaping alloc (returned from the function) should NOT be promoted.
    #[test]
    fn test_sroa_skips_escaping_alloc() {
        // %v1 = alloc 8
        // ret %v1
        let instrs = vec![IRInstr::Alloc {
            dst: r(1),
            size: 8,
        }];
        let mut func = fn_with("sroa_escape", instrs, IRTerminator::Return(vec![r(1)]));

        let info = analyze_escapes(&func);
        assert_eq!(info.get(&1).copied(), Some(EscapeResult::Escapes));

        let promoted = scalar_replace_aggregates(&mut func, &info);
        assert_eq!(promoted, 0, "escaping alloc must not be promoted");
        // Alloc must still be present.
        let alloc_present = func.blocks[0]
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstr::Alloc { .. }));
        assert!(alloc_present);
    }

    /// Alloc with no accesses at all → SROA skips it (alloc-elision handles it).
    #[test]
    fn test_sroa_skips_unused_alloc() {
        let instrs = vec![IRInstr::Alloc {
            dst: r(1),
            size: 8,
        }];
        let mut func = fn_with("sroa_unused", instrs, IRTerminator::Return(vec![]));

        let info = analyze_escapes(&func);
        let promoted = scalar_replace_aggregates(&mut func, &info);
        assert_eq!(promoted, 0);
    }

    // ── Alloc-elision tests ───────────────────────────────────────────

    /// Non-escaping `__vuma_alloc` call with no Loads/Stores, followed
    /// by a matching `__vuma_free`: both should be removed.
    #[test]
    fn test_elide_non_escaping_heap_alloc() {
        // %v1 = call __vuma_alloc(16)
        // call __vuma_free(%v1)
        // ret
        let instrs = vec![
            IRInstr::Call {
                dst: Some(r(1)),
                func: "__vuma_alloc".to_string(),
                args: vec![IRValue::Immediate(16)],
                is_extern: true,
            },
            IRInstr::Call {
                dst: None,
                func: "__vuma_free".to_string(),
                args: vec![r(1)],
                is_extern: true,
            },
        ];
        let mut func = fn_with("elide", instrs, IRTerminator::Return(vec![]));

        let info = analyze_escapes(&func);
        assert_eq!(
            info.get(&1).copied(),
            Some(EscapeResult::DoesNotEscape),
            "alloc should be non-escaping"
        );

        let elided = elide_non_escaping_allocs(&mut func, &info);
        assert_eq!(elided, 1, "one alloc pair should be elided");

        // No calls should remain.
        let calls_remaining: usize = func
            .blocks
            .iter()
            .map(|b| b.instructions.iter().filter(|i| matches!(i, IRInstr::Call { .. })).count())
            .sum();
        assert_eq!(calls_remaining, 0, "both alloc and free calls should be removed");
    }

    /// Alloc that escapes (passed to another Call) should NOT be elided.
    #[test]
    fn test_elide_skips_escaping_alloc() {
        // %v1 = call __vuma_alloc(16)
        // call escape(%v1)        -- v1 escapes
        // call __vuma_free(%v1)
        let instrs = vec![
            IRInstr::Call {
                dst: Some(r(1)),
                func: "__vuma_alloc".to_string(),
                args: vec![IRValue::Immediate(16)],
                is_extern: true,
            },
            IRInstr::Call {
                dst: None,
                func: "escape".to_string(),
                args: vec![r(1)],
                is_extern: true,
            },
            IRInstr::Call {
                dst: None,
                func: "__vuma_free".to_string(),
                args: vec![r(1)],
                is_extern: true,
            },
        ];
        let mut func = fn_with("elide_escape", instrs, IRTerminator::Return(vec![]));

        let info = analyze_escapes(&func);
        assert_eq!(info.get(&1).copied(), Some(EscapeResult::Escapes));

        let elided = elide_non_escaping_allocs(&mut func, &info);
        assert_eq!(elided, 0, "escaping alloc must not be elided");
        // All three calls should remain.
        assert_eq!(
            func.blocks[0]
                .instructions
                .iter()
                .filter(|i| matches!(i, IRInstr::Call { .. }))
                .count(),
            3
        );
    }

    /// Non-escaping alloc that IS accessed (Store) should NOT be elided.
    #[test]
    fn test_elide_skips_accessed_alloc() {
        // %v1 = call __vuma_alloc(16)
        // store 42 -> (%v1, 0)
        // call __vuma_free(%v1)
        let instrs = vec![
            IRInstr::Call {
                dst: Some(r(1)),
                func: "__vuma_alloc".to_string(),
                args: vec![IRValue::Immediate(16)],
                is_extern: true,
            },
            IRInstr::Store {
                value: IRValue::Immediate(42),
                addr: r(1),
                offset: 0,
                ty: IRType::I64,
            },
            IRInstr::Call {
                dst: None,
                func: "__vuma_free".to_string(),
                args: vec![r(1)],
                is_extern: true,
            },
        ];
        let mut func = fn_with("elide_accessed", instrs, IRTerminator::Return(vec![]));

        let info = analyze_escapes(&func);
        assert_eq!(info.get(&1).copied(), Some(EscapeResult::DoesNotEscape));

        let elided = elide_non_escaping_allocs(&mut func, &info);
        assert_eq!(elided, 0, "accessed alloc must not be elided");
    }

    // ── Program-wide wrapper sanity test ─────────────────────────────

    #[test]
    fn test_analyze_escapes_program_two_functions() {
        // fn f: %v1 = alloc 8; ret %v1   (escapes)
        // fn g: %v1 = alloc 8;           (does not escape)
        let mut f = IRFunction::new("f".to_string());
        f.blocks[0].instructions.push(IRInstr::Alloc {
            dst: r(1),
            size: 8,
        });
        f.blocks[0].terminator = IRTerminator::Return(vec![r(1)]);

        let mut g = IRFunction::new("g".to_string());
        g.blocks[0].instructions.push(IRInstr::Alloc {
            dst: r(1),
            size: 8,
        });
        g.blocks[0].terminator = IRTerminator::Return(vec![]);

        let map = analyze_escapes_program(&[f, g]);
        assert_eq!(map["f"][&1], EscapeResult::Escapes);
        assert_eq!(map["g"][&1], EscapeResult::DoesNotEscape);
    }

    // ── Syscall escape regression tests (P1-c-2) ──────────────────────
    //
    // A syscall that takes a pointer argument (e.g. write(fd, buf, n))
    // reads/writes through that pointer in the kernel. The allocation
    // MUST be treated as escaping so it is not elided by SROA/alloc-elision.
    // Before the fix, IRInstr::Syscall was missing from the escape-point
    // analysis, causing the alloc + stores + syscall to be removed at O2.

    /// Alloc passed to a Syscall as an argument escapes (kernel reads it).
    #[test]
    fn test_syscall_arg_alloc_escapes() {
        // %v1 = call __vuma_alloc(3)
        // store 72 -> [%v1 + 0]
        // %v2 = syscall(64, 1, %v1, 3)   // write(1, v1, 3)
        // call __vuma_free(%v1)
        // ret
        let instrs = vec![
            IRInstr::Call {
                dst: Some(r(1)),
                func: "__vuma_alloc".to_string(),
                args: vec![IRValue::Immediate(3)],
                is_extern: true,
            },
            IRInstr::Store {
                value: IRValue::Immediate(72),
                addr: r(1),
                offset: 0,
                ty: IRType::U8,
            },
            IRInstr::Syscall {
                nr: 64,
                args: vec![
                    IRValue::Immediate(1),
                    r(1),
                    IRValue::Immediate(3),
                ],
                dst: Some(r(2)),
            },
            IRInstr::Call {
                dst: None,
                func: "__vuma_free".to_string(),
                args: vec![r(1)],
                is_extern: true,
            },
        ];
        let func = fn_with("syscall_escape", instrs, IRTerminator::Return(vec![]));

        let info = analyze_escapes(&func);
        assert_eq!(
            info.get(&1).copied(),
            Some(EscapeResult::Escapes),
            "alloc passed to syscall MUST escape — kernel reads through it"
        );
    }

    /// Alloc passed to a Syscall must NOT be elided.
    #[test]
    fn test_elide_skips_syscall_escape() {
        let instrs = vec![
            IRInstr::Call {
                dst: Some(r(1)),
                func: "__vuma_alloc".to_string(),
                args: vec![IRValue::Immediate(3)],
                is_extern: true,
            },
            IRInstr::Syscall {
                nr: 64,
                args: vec![IRValue::Immediate(1), r(1), IRValue::Immediate(3)],
                dst: Some(r(2)),
            },
            IRInstr::Call {
                dst: None,
                func: "__vuma_free".to_string(),
                args: vec![r(1)],
                is_extern: true,
            },
        ];
        let mut func = fn_with("elide_syscall", instrs, IRTerminator::Return(vec![]));

        let info = analyze_escapes(&func);
        let elided = elide_non_escaping_allocs(&mut func, &info);
        assert_eq!(
            elided, 0,
            "alloc passed to syscall must NOT be elided"
        );
    }

    /// P2 regression: allocate() lowers to Syscall{nr:222 (mmap)}. The mmap
    /// result vreg must be recognized as an allocation by analyze_escapes,
    /// and when passed to another syscall (e.g. write) it must be marked
    /// as escaping (not elided).
    #[test]
    fn test_mmap_syscall_alloc_escapes() {
        // %v1 = syscall(222, 0, 3, 3, 0x22, -1, 0)   // mmap(NULL, 3, RW, PRIVATE|ANON, -1, 0)
        // store 72 -> [%v1 + 0]
        // %v2 = syscall(64, 1, %v1, 3)               // write(1, v1, 3)
        // syscall(215, %v1, 0)                        // munmap(%v1, 0)
        // ret
        let instrs = vec![
            IRInstr::Syscall {
                nr: 222,
                args: vec![
                    IRValue::Immediate(0),
                    IRValue::Immediate(3),
                    IRValue::Immediate(3),
                    IRValue::Immediate(0x22),
                    IRValue::Immediate(-1),
                    IRValue::Immediate(0),
                ],
                dst: Some(r(1)),
            },
            IRInstr::Store {
                value: IRValue::Immediate(72),
                addr: r(1),
                offset: 0,
                ty: IRType::U8,
            },
            IRInstr::Syscall {
                nr: 64,
                args: vec![IRValue::Immediate(1), r(1), IRValue::Immediate(3)],
                dst: Some(r(2)),
            },
            IRInstr::Syscall {
                nr: 215,
                args: vec![r(1), IRValue::Immediate(0)],
                dst: None,
            },
        ];
        let func = fn_with("mmap_escape", instrs, IRTerminator::Return(vec![]));

        let info = analyze_escapes(&func);
        assert_eq!(
            info.get(&1).copied(),
            Some(EscapeResult::Escapes),
            "mmap-allocated buffer passed to write syscall MUST escape"
        );
    }

    // ── Wave 3 regression tests: pointer-arithmetic aliasing ──────────
    //
    // The root cause of the Wave 2 SIGSEGV regressions on mem_copy_buffer,
    // doubly_linked_list, mf_address_return, etc. was that the previous
    // escape analysis only checked direct Load/Store with addr == alloc,
    // missing accesses through derived aliases (alloc + i computed via Add,
    // then used as a Store addr). The alloc was marked DoesNotEscape and
    // incorrectly elided, leaving the Add/Store referencing a removed vreg.

    /// Alloc whose address flows through an `Add` (pointer arithmetic)
    /// MUST be marked as escaping — the derived alias may be stored,
    /// returned, or passed to a call later, and we cannot track aliases
    /// soundly without a full alias analysis.
    #[test]
    fn test_alloc_used_in_add_escapes() {
        // %v1 = alloc 16
        // %v2 = add %v1, 0          (alias of %v1)
        // store 42 -> [%v2 + 0]     (store through alias)
        // free %v1
        // ret
        let instrs = vec![
            IRInstr::Alloc {
                dst: r(1),
                size: 16,
            },
            IRInstr::Add {
                dst: r(2),
                lhs: r(1),
                rhs: IRValue::Immediate(0),
                ty: Some(IRType::I64),
            },
            IRInstr::Store {
                value: IRValue::Immediate(42),
                addr: r(2),
                offset: 0,
                ty: IRType::U8,
            },
            IRInstr::Free { ptr: r(1) },
        ];
        let func = fn_with("add_alias", instrs, IRTerminator::Return(vec![]));

        let info = analyze_escapes(&func);
        assert_eq!(
            info.get(&1).copied(),
            Some(EscapeResult::Escapes),
            "alloc used in Add (pointer arithmetic) MUST escape — the \
             derived alias may be observed later"
        );
    }

    /// Alloc whose address flows through an `Add` MUST NOT be elided,
    /// even though the only direct Load/Store is on the alias (not on the
    /// alloc vreg itself). This is the exact pattern that caused the Wave
    /// 2 SIGSEGV regressions.
    #[test]
    fn test_elide_skips_alloc_with_add_alias() {
        // %v1 = call __vuma_alloc(16)
        // %v2 = add %v1, 0           (alias of %v1)
        // store 42 -> [%v2 + 0]      (store through alias — NOT direct on v1)
        // call __vuma_free(%v1)
        // ret
        let instrs = vec![
            IRInstr::Call {
                dst: Some(r(1)),
                func: "__vuma_alloc".to_string(),
                args: vec![IRValue::Immediate(16)],
                is_extern: true,
            },
            IRInstr::Add {
                dst: r(2),
                lhs: r(1),
                rhs: IRValue::Immediate(0),
                ty: Some(IRType::I64),
            },
            IRInstr::Store {
                value: IRValue::Immediate(42),
                addr: r(2),
                offset: 0,
                ty: IRType::U8,
            },
            IRInstr::Call {
                dst: None,
                func: "__vuma_free".to_string(),
                args: vec![r(1)],
                is_extern: true,
            },
        ];
        let mut func = fn_with("elide_add_alias", instrs, IRTerminator::Return(vec![]));

        let info = analyze_escapes(&func);
        assert_eq!(
            info.get(&1).copied(),
            Some(EscapeResult::Escapes),
            "alloc used in Add MUST be marked escaping so it is not elided"
        );

        let elided = elide_non_escaping_allocs(&mut func, &info);
        assert_eq!(
            elided, 0,
            "alloc with Add-aliased accesses MUST NOT be elided — the \
             Add still references the alloc vreg"
        );

        // The Alloc and Free must both still be present.
        let has_alloc = func
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .any(|i| matches!(i, IRInstr::Call { func, .. } if func == "__vuma_alloc"));
        assert!(has_alloc, "alloc call must NOT be removed");
    }

    /// SROA must NOT promote an alloc whose address is used in an `Add`
    /// (pointer arithmetic), even if there are also direct Load/Store
    /// accesses on the alloc. The Add creates an alias that SROA cannot
    /// soundly rewrite.
    #[test]
    fn test_sroa_skips_alloc_with_add_alias() {
        // %v1 = alloc 16
        // %v2 = add %v1, 0           (alias — SROA must bail)
        // store 42 -> [%v1 + 0]      (direct store)
        // %v3 = load [%v1 + 0]       (direct load)
        // free %v1
        // ret %v3
        let instrs = vec![
            IRInstr::Alloc {
                dst: r(1),
                size: 16,
            },
            IRInstr::Add {
                dst: r(2),
                lhs: r(1),
                rhs: IRValue::Immediate(0),
                ty: Some(IRType::I64),
            },
            IRInstr::Store {
                value: IRValue::Immediate(42),
                addr: r(1),
                offset: 0,
                ty: IRType::I64,
            },
            IRInstr::Load {
                dst: r(3),
                addr: r(1),
                offset: 0,
                ty: IRType::I64,
            },
            IRInstr::Free { ptr: r(1) },
        ];
        let mut func = fn_with("sroa_add_alias", instrs, IRTerminator::Return(vec![r(3)]));

        // analyze_escapes marks this as Escapes (because of the Add),
        // so SROA's non_escaping filter excludes it. SROA promotes 0.
        let info = analyze_escapes(&func);
        assert_eq!(info.get(&1).copied(), Some(EscapeResult::Escapes));
        let promoted = scalar_replace_aggregates(&mut func, &info);
        assert_eq!(promoted, 0, "alloc used in Add must NOT be SROA-promoted");

        // Even if we force the alloc to be DoesNotEscape, SROA's
        // defense-in-depth check should still bail on the Add.
        let mut info_forced = HashMap::new();
        info_forced.insert(1u32, EscapeResult::DoesNotEscape);
        let promoted_forced = scalar_replace_aggregates(&mut func, &info_forced);
        assert_eq!(
            promoted_forced, 0,
            "SROA defense-in-depth must bail on Add alias even if \
             escape_info incorrectly says DoesNotEscape"
        );

        // Alloc must still be present.
        let has_alloc = func
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .any(|i| matches!(i, IRInstr::Alloc { .. }));
        assert!(has_alloc, "Alloc must NOT be removed");
    }

    // Reference the unused import to silence dead-code warnings in
    // single-test configurations.
    #[test]
    fn _ensure_irblock_imported() {
        let _ = IRBlock::new("placeholder");
    }
}
