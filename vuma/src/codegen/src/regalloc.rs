//! # Register Allocation
//!
//! A simple register allocator that maps IR virtual registers to ARM64
//! physical registers.
//!
//! ## Strategy
//!
//! The current implementation uses a **greedy linear-scan** approach:
//!
//! 1. Walk the IR instructions in order.
//! 2. For each virtual register that needs a physical home, grab the first
//!    free caller-saved register (X0–X15, X17).
//! 3. If no free register is available, **spill** the least-recently-used
//!    register to the stack and free it.
//! 4. Callee-saved registers (X19–X28) are reserved for values that live
//!    across function calls.
//!
//! This is intentionally simple — a proper graph-coloring or SSA-based
//! allocator can be swapped in later.

use std::collections::HashMap;

use crate::arm64::Register;
use crate::ir::{IRFunction, IRProgram, IRValue};
use crate::CodegenError;
use crate::Result;

/// ID type for IR virtual registers.
pub type IRValueId = u32;

// ---------------------------------------------------------------------------
// RegAllocator
// ---------------------------------------------------------------------------

/// A simple greedy register allocator.
///
/// Maintains a pool of free physical registers and a mapping from virtual
/// register IDs to physical registers.  When the pool is exhausted, values
/// are spilled to the stack.
#[derive(Debug)]
pub struct RegAllocator {
    /// Physical registers available for allocation (caller-saved by default).
    free_regs: Vec<Register>,
    /// Current mapping: virtual register ID → physical register.
    used_regs: HashMap<IRValueId, Register>,
    /// Spill slot counter (each slot is 8 bytes on the stack).
    next_spill_slot: u32,
    /// Records which virtual registers have been spilled and their stack
    /// slot offset.
    spill_map: HashMap<IRValueId, u32>,
    /// Callee-saved registers reserved for cross-call values.
    callee_saved_pool: Vec<Register>,
    /// Track which callee-saved registers are in use.
    callee_saved_used: HashMap<IRValueId, Register>,
}

impl RegAllocator {
    /// Create a new allocator with the default ARM64 caller-saved register
    /// pool.
    ///
    /// The following registers are **not** in the general pool:
    /// - `X8` (indirect result location)
    /// - `X16` (IP0 — linker veneer)
    /// - `X18` (platform register)
    /// - `X29` (frame pointer)
    /// - `X30` (link register)
    /// - `SP`, `XZR`
    pub fn new() -> Self {
        let free_regs = vec![
            Register::X0,
            Register::X1,
            Register::X2,
            Register::X3,
            Register::X4,
            Register::X5,
            Register::X6,
            Register::X7,
            Register::X9,
            Register::X10,
            Register::X11,
            Register::X12,
            Register::X13,
            Register::X14,
            Register::X15,
            Register::X17,
        ];

        let callee_saved_pool = vec![
            Register::X19,
            Register::X20,
            Register::X21,
            Register::X22,
            Register::X23,
            Register::X24,
            Register::X25,
            Register::X26,
            Register::X27,
            Register::X28,
        ];

        Self {
            free_regs,
            used_regs: HashMap::new(),
            next_spill_slot: 0,
            spill_map: HashMap::new(),
            callee_saved_pool,
            callee_saved_used: HashMap::new(),
        }
    }

    /// Allocate a physical register for the given virtual register ID.
    ///
    /// If a mapping already exists, returns the existing register.
    /// Otherwise, picks a free register from the pool.  If no free register
    /// is available, spills a value and returns the freed register.
    pub fn allocate(&mut self, vreg: IRValueId) -> Result<Register> {
        // Already allocated?
        if let Some(&reg) = self.used_regs.get(&vreg) {
            return Ok(reg);
        }

        // Try to grab a free register.
        if let Some(reg) = self.free_regs.pop() {
            self.used_regs.insert(vreg, reg);
            return Ok(reg);
        }

        // No free caller-saved registers — try callee-saved.
        if let Some(reg) = self.callee_saved_pool.pop() {
            self.callee_saved_used.insert(vreg, reg);
            return Ok(reg);
        }

        // Still nothing — spill the least recently used value.
        self.spill()?;
        // Try again.
        if let Some(reg) = self.free_regs.pop() {
            self.used_regs.insert(vreg, reg);
            return Ok(reg);
        }

        Err(CodegenError::RegisterAllocFailed(format!(
            "cannot allocate a register for vreg {} — all registers exhausted",
            vreg
        )))
    }

    /// Allocate a callee-saved register (for values that must survive
    /// function calls).
    pub fn allocate_callee_saved(&mut self, vreg: IRValueId) -> Result<Register> {
        if let Some(&reg) = self.callee_saved_used.get(&vreg) {
            return Ok(reg);
        }
        if let Some(&reg) = self.used_regs.get(&vreg) {
            return Ok(reg);
        }
        if let Some(reg) = self.callee_saved_pool.pop() {
            self.callee_saved_used.insert(vreg, reg);
            return Ok(reg);
        }
        Err(CodegenError::RegisterAllocFailed(format!(
            "no callee-saved register available for vreg {}",
            vreg
        )))
    }

    /// Free a physical register previously allocated to `vreg`.
    ///
    /// Returns the register to the appropriate free pool.
    pub fn free(&mut self, vreg: IRValueId) {
        if let Some(reg) = self.used_regs.remove(&vreg) {
            self.free_regs.push(reg);
        }
        if let Some(reg) = self.callee_saved_used.remove(&vreg) {
            self.callee_saved_pool.push(reg);
        }
        // Also remove from spill map if it was spilled earlier.
        self.spill_map.remove(&vreg);
    }

    /// Spill the oldest (first-inserted) mapped register to the stack.
    ///
    /// This is a simple eviction strategy.  The spill slot offset is stored
    /// in `spill_map` for later reload.
    pub fn spill(&mut self) -> Result<()> {
        // Find the first entry in used_regs to evict.
        let vreg_to_spill = self
            .used_regs
            .keys()
            .copied()
            .next()
            .ok_or_else(|| CodegenError::RegisterAllocFailed("no register to spill".into()))?;

        let reg = self.used_regs.remove(&vreg_to_spill).unwrap();
        let slot = self.next_spill_slot;
        self.next_spill_slot += 1;
        self.spill_map.insert(vreg_to_spill, slot);
        self.free_regs.push(reg);

        log::debug!("spilled vreg {} to stack slot {} (freed {})", vreg_to_spill, slot, reg);
        Ok(())
    }

    /// Look up the physical register for a virtual register, allocating one
    /// if necessary.
    pub fn get_or_alloc(&mut self, vreg: IRValueId) -> Result<Register> {
        self.allocate(vreg)
    }

    /// Get the physical register for a virtual register, if it has already
    /// been allocated (returns `None` if not yet allocated).
    pub fn get(&self, vreg: IRValueId) -> Option<Register> {
        self.used_regs
            .get(&vreg)
            .copied()
            .or_else(|| self.callee_saved_used.get(&vreg).copied())
    }

    /// Check whether a virtual register has been spilled.
    pub fn is_spilled(&self, vreg: IRValueId) -> bool {
        self.spill_map.contains_key(&vreg)
    }

    /// Get the spill slot offset (in units of 8 bytes) for a spilled vreg.
    pub fn spill_slot(&self, vreg: IRValueId) -> Option<u32> {
        self.spill_map.get(&vreg).copied()
    }

    /// Total number of spill slots currently in use.
    pub fn spill_count(&self) -> u32 {
        self.next_spill_slot
    }

    /// Returns the set of callee-saved registers that are in use.
    /// The function prologue/epilogue must save/restore these.
    pub fn used_callee_saved(&self) -> Vec<Register> {
        self.callee_saved_used.values().copied().collect()
    }

    /// Reset the allocator state (e.g. between functions).
    pub fn reset(&mut self) {
        // Restore default pools.
        *self = Self::new();
    }

    // ---- Program-level helpers ----

    /// Run allocation over an entire IR program, returning a per-function
    /// mapping from virtual register IDs to physical registers.
    ///
    /// This is a convenience method; for fine-grained control, call
    /// `allocate()` directly while iterating IR instructions.
    pub fn allocate_program(&mut self, program: &IRProgram) -> Result<HashMap<IRValueId, Register>> {
        let mut all_mappings = HashMap::new();

        for func in &program.functions {
            self.reset();
            let func_mappings = self.allocate_function(func)?;
            all_mappings.extend(func_mappings);
        }

        Ok(all_mappings)
    }

    /// Run allocation over a single IR function.
    pub fn allocate_function(&mut self, func: &IRFunction) -> Result<HashMap<IRValueId, Register>> {
        for block in &func.blocks {
            for instr in &block.instructions {
                // Allocate registers for all used virtual registers.
                for vreg_id in instr.used_regs() {
                    self.allocate(vreg_id)?;
                }
                // Allocate registers for all defined virtual registers.
                for vreg_id in instr.defined_regs() {
                    self.allocate(vreg_id)?;
                }
            }
        }

        // Collect the final mapping.
        let mut mappings = HashMap::new();
        for (&vreg, &reg) in &self.used_regs {
            mappings.insert(vreg, reg);
        }
        for (&vreg, &reg) in &self.callee_saved_used {
            mappings.insert(vreg, reg);
        }
        Ok(mappings)
    }

    /// Resolve an [`IRValue`] to a physical register or immediate.
    ///
    /// - `Register(id)` → allocate and return the physical register.
    /// - `Immediate(v)` → `Err` (immediates don't need registers; the
    ///   caller should handle them directly in the instruction encoding).
    pub fn resolve_value(&mut self, val: &IRValue) -> Result<Option<Register>> {
        match val {
            IRValue::Register(id) => Ok(Some(self.allocate(*id)?)),
            IRValue::Immediate(_) => Ok(None),
            IRValue::Address(_) => {
                // TODO: load address into a register via MOVZ/MOVK sequence
                Ok(None)
            }
            IRValue::Label(_) => Ok(None),
        }
    }
}

impl Default for RegAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_basic() {
        let mut alloc = RegAllocator::new();
        let r0 = alloc.allocate(0).unwrap();
        let r1 = alloc.allocate(1).unwrap();
        assert_ne!(r0, r1);
    }

    #[test]
    fn free_and_reuse() {
        let mut alloc = RegAllocator::new();
        let r0 = alloc.allocate(0).unwrap();
        alloc.free(0);
        let r1 = alloc.allocate(1).unwrap();
        // The freed register should be reused.
        assert_eq!(r0, r1);
    }

    #[test]
    fn spill_when_exhausted() {
        let mut alloc = RegAllocator::new();
        // Allocate until we run out of free registers.
        for i in 0..30 {
            let result = alloc.allocate(i);
            // Should succeed even if spilling occurs.
            assert!(result.is_ok(), "allocation for vreg {} failed", i);
        }
        assert!(alloc.spill_count() > 0, "expected some spills");
    }

    #[test]
    fn callee_saved_tracking() {
        let mut alloc = RegAllocator::new();
        let _ = alloc.allocate_callee_saved(0).unwrap();
        let saved = alloc.used_callee_saved();
        assert!(!saved.is_empty());
    }

    #[test]
    fn allocate_function() {
        use crate::ir::{BinOpKind, IRInstr, IRTerminator};

        let mut func = crate::ir::IRFunction::new("test");
        func.params.push(IRValue::Register(0));
        func.params.push(IRValue::Register(1));
        let block = func.current_block();
        block.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(2),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(1),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let mut alloc = RegAllocator::new();
        let mappings = alloc.allocate_function(&func).unwrap();
        assert!(mappings.contains_key(&0));
        assert!(mappings.contains_key(&1));
        assert!(mappings.contains_key(&2));
    }
}
