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

/// Analyze a function for escaping allocations.
///
/// Returns a map from vreg (allocation result) to escape result.
/// Both `IRInstr::Alloc` (stack) and `IRInstr::Call` to
/// `__vuma_alloc`/`allocate` (heap) are tracked as allocations.
pub fn analyze_escapes(func: &IRFunction) -> HashMap<u32, EscapeResult> {
    let mut allocs: HashSet<u32> = HashSet::new();
    let mut escapes: HashSet<u32> = HashSet::new();

    // Phase 1: Find all allocations (stack Alloc + heap alloc calls).
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
                _ => {}
            }
        }
    }

    // Phase 2: Find escape points
    for block in &func.blocks {
        for instr in &block.instructions {
            match instr {
                // Store: if value is an allocation, it escapes
                IRInstr::Store { value, .. } => {
                    if let IRValue::Register(vreg) = value {
                        if allocs.contains(vreg) {
                            escapes.insert(*vreg);
                        }
                    }
                }

                // Call: if any argument is an allocation, it escapes
                // (unless the call is to free())
                IRInstr::Call { args, func: fname, .. }
                    if !is_free_call(fname) => {
                        for arg in args {
                            if let IRValue::Register(vreg) = arg {
                                if allocs.contains(vreg) {
                                    escapes.insert(*vreg);
                                }
                            }
                        }
                    }

                // Phi: if any incoming is an escaping allocation,
                // mark the phi result as escaping
                IRInstr::Phi { dst, incoming } => {
                    if let Some(phi_vreg) = dst.as_register() {
                        for (val, _) in incoming {
                            if let IRValue::Register(src_vreg) = val {
                                if escapes.contains(src_vreg) {
                                    escapes.insert(phi_vreg);
                                }
                            }
                        }
                    }
                }

                _ => {}
            }
        }

        // Check terminator for return
        if let IRTerminator::Return(vals) = &block.terminator {
            for val in vals {
                if let IRValue::Register(vreg) = val {
                    if allocs.contains(vreg) {
                        escapes.insert(*vreg);
                    }
                }
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
                        if let (Some(val_v), Some(addr_v)) =
                            (value.as_register(), addr.as_register())
                        {
                            if addr_v == alloc_vreg {
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
                    _ => {}
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
/// - If any Load/Store accesses `A` directly, skip (the memory IS
///   used — SROA should have handled it).
/// - Otherwise, remove the producing `Alloc` or `__vuma_alloc` Call,
///   and remove the matching `Free` / `__vuma_free` / `free` Call that
///   takes `A` as its sole argument.
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
        // Bail if the memory is accessed at all.
        let mut has_access = false;
        for block in &func.blocks {
            for instr in &block.instructions {
                match instr {
                    IRInstr::Load { addr, .. } | IRInstr::Store { addr, .. }
                        if addr.as_register() == Some(alloc_vreg) => {
                            has_access = true;
                        }
                    IRInstr::AtomicLoad { addr, .. } | IRInstr::AtomicStore { addr, .. }
                        if addr.as_register() == Some(alloc_vreg) => {
                            has_access = true;
                        }
                    _ => {}
                }
                if has_access {
                    break;
                }
            }
            if has_access {
                break;
            }
        }
        if has_access {
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

    // Reference the unused import to silence dead-code warnings in
    // single-test configurations.
    #[test]
    fn _ensure_irblock_imported() {
        let _ = IRBlock::new("placeholder");
    }
}
