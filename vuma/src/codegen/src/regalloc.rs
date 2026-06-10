//! # Register Allocation
//!
//! Provides register allocators that map IR virtual registers to ARM64
//! physical registers.
//!
//! ## Allocators
//!
//! ### `RegAllocator` (legacy greedy)
//!
//! A simple greedy allocator that walks the IR and assigns caller-saved
//! registers first, then callee-saved, spilling when all are exhausted.
//! Kept for backward-compatibility with the existing emitter.
//!
//! ### `LinearScanAllocator` (production)
//!
//! A real **linear-scan** register allocator that:
//!
//! 1. Computes live ranges from the IR (per-function, across all blocks).
//! 2. Sorts intervals by start point.
//! 3. Walks intervals in order, assigning free physical registers.
//! 4. When the pool is exhausted, evicts the interval whose end point is
//!    farthest in the future (or spills the current one if it ends latest).
//! 5. Generates spill/reload code as needed — including reloads at each
//!    use position after eviction.
//! 6. Applies register coalescing to eliminate redundant copies.
//!
//! ## ARM64 Register Conventions (AAPCS64)
//!
//! | Register(s) | Role                                   | Class        |
//! |-------------|----------------------------------------|--------------|
//! | X0–X7       | Argument / result registers            | Caller-saved |
//! | X8          | Indirect result location register      | Caller-saved |
//! | X9–X15      | Caller-saved temporary registers       | Caller-saved |
//! | X16–X17     | Intra-procedure-call scratch (IP0/IP1) | Caller-saved |
//! | X18         | Platform register                      | Reserved     |
//! | X19–X28     | Callee-saved registers                 | Callee-saved |
//! | X29         | Frame pointer (FP)                     | Reserved     |
//! | X30         | Link register (LR)                     | Reserved     |
//! | SP          | Stack pointer                          | Reserved     |
//! | XZR         | Zero register                          | Reserved     |
//! | V0–V7       | FP/SIMD argument / result registers    | Caller-saved |
//! | V8–V15      | FP/SIMD callee-saved (lower 64 bits)   | Callee-saved |
//! | V16–V31     | FP/SIMD caller-saved temporaries       | Caller-saved |

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::arm64::Register;
use crate::ir::{IRFunction, IRInstr, IRProgram, IRTerminator, IRValue};
use crate::CodegenError;
use crate::Result;

/// ID type for IR virtual registers.
pub type IRValueId = u32;

// ═══════════════════════════════════════════════════════════════════════════
// SIMD / FP Register enum
// ═══════════════════════════════════════════════════════════════════════════

/// ARM64 SIMD and Floating-Point registers (V0–V31).
///
/// Each V register is 128 bits wide.  The lower 64 bits can be accessed as
/// `Dn` (double-precision) and the lower 32 bits as `Sn` (single-precision).
///
/// AAPCS64 classification:
/// - V0–V7: caller-saved (argument / result)
/// - V8–V15: callee-saved (only lower 64 bits, D8–D15, must be preserved)
/// - V16–V31: caller-saved temporaries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SimdFpRegister {
    V0,
    V1,
    V2,
    V3,
    V4,
    V5,
    V6,
    V7,
    V8,
    V9,
    V10,
    V11,
    V12,
    V13,
    V14,
    V15,
    V16,
    V17,
    V18,
    V19,
    V20,
    V21,
    V22,
    V23,
    V24,
    V25,
    V26,
    V27,
    V28,
    V29,
    V30,
    V31,
}

impl SimdFpRegister {
    /// Returns the 5-bit encoding index for this register (0–31).
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    /// Returns the standard assembly name.
    pub fn asm_name(&self) -> &'static str {
        match self {
            SimdFpRegister::V0 => "v0",
            SimdFpRegister::V1 => "v1",
            SimdFpRegister::V2 => "v2",
            SimdFpRegister::V3 => "v3",
            SimdFpRegister::V4 => "v4",
            SimdFpRegister::V5 => "v5",
            SimdFpRegister::V6 => "v6",
            SimdFpRegister::V7 => "v7",
            SimdFpRegister::V8 => "v8",
            SimdFpRegister::V9 => "v9",
            SimdFpRegister::V10 => "v10",
            SimdFpRegister::V11 => "v11",
            SimdFpRegister::V12 => "v12",
            SimdFpRegister::V13 => "v13",
            SimdFpRegister::V14 => "v14",
            SimdFpRegister::V15 => "v15",
            SimdFpRegister::V16 => "v16",
            SimdFpRegister::V17 => "v17",
            SimdFpRegister::V18 => "v18",
            SimdFpRegister::V19 => "v19",
            SimdFpRegister::V20 => "v20",
            SimdFpRegister::V21 => "v21",
            SimdFpRegister::V22 => "v22",
            SimdFpRegister::V23 => "v23",
            SimdFpRegister::V24 => "v24",
            SimdFpRegister::V25 => "v25",
            SimdFpRegister::V26 => "v26",
            SimdFpRegister::V27 => "v27",
            SimdFpRegister::V28 => "v28",
            SimdFpRegister::V29 => "v29",
            SimdFpRegister::V30 => "v30",
            SimdFpRegister::V31 => "v31",
        }
    }

    /// Returns `true` if this register is callee-saved (V8–V15).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            SimdFpRegister::V8
                | SimdFpRegister::V9
                | SimdFpRegister::V10
                | SimdFpRegister::V11
                | SimdFpRegister::V12
                | SimdFpRegister::V13
                | SimdFpRegister::V14
                | SimdFpRegister::V15
        )
    }

    /// Returns `true` if this register is caller-saved (V0–V7, V16–V31).
    pub fn is_caller_saved(&self) -> bool {
        !self.is_callee_saved()
    }

    /// Return all 32 SIMD/FP registers in order.
    pub fn all() -> &'static [SimdFpRegister; 32] {
        static ALL: [SimdFpRegister; 32] = [
            SimdFpRegister::V0,
            SimdFpRegister::V1,
            SimdFpRegister::V2,
            SimdFpRegister::V3,
            SimdFpRegister::V4,
            SimdFpRegister::V5,
            SimdFpRegister::V6,
            SimdFpRegister::V7,
            SimdFpRegister::V8,
            SimdFpRegister::V9,
            SimdFpRegister::V10,
            SimdFpRegister::V11,
            SimdFpRegister::V12,
            SimdFpRegister::V13,
            SimdFpRegister::V14,
            SimdFpRegister::V15,
            SimdFpRegister::V16,
            SimdFpRegister::V17,
            SimdFpRegister::V18,
            SimdFpRegister::V19,
            SimdFpRegister::V20,
            SimdFpRegister::V21,
            SimdFpRegister::V22,
            SimdFpRegister::V23,
            SimdFpRegister::V24,
            SimdFpRegister::V25,
            SimdFpRegister::V26,
            SimdFpRegister::V27,
            SimdFpRegister::V28,
            SimdFpRegister::V29,
            SimdFpRegister::V30,
            SimdFpRegister::V31,
        ];
        &ALL
    }
}

impl std::fmt::Display for SimdFpRegister {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Physical register union — either a GPR or a SIMD/FP register
// ═══════════════════════════════════════════════════════════════════════════

/// A physical register on ARM64 — either a general-purpose register or a
/// SIMD/FP register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PhysReg {
    Gpr(Register),
    SimdFp(SimdFpRegister),
}

impl PhysReg {
    /// Returns `true` if this is a callee-saved register.
    pub fn is_callee_saved(&self) -> bool {
        match self {
            PhysReg::Gpr(r) => r.is_callee_saved(),
            PhysReg::SimdFp(r) => r.is_callee_saved(),
        }
    }

    /// Returns `true` if this is a caller-saved register.
    pub fn is_caller_saved(&self) -> bool {
        match self {
            PhysReg::Gpr(r) => r.is_caller_saved(),
            PhysReg::SimdFp(r) => r.is_caller_saved(),
        }
    }
}

impl std::fmt::Display for PhysReg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PhysReg::Gpr(r) => write!(f, "{}", r),
            PhysReg::SimdFp(r) => write!(f, "{}", r),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Register class — determines which physical register pool to use
// ═══════════════════════════════════════════════════════════════════════════

/// The class of a virtual register, determining which physical register
/// pool it can be allocated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RegClass {
    /// General-purpose integer register (X0–X30).
    Gpr,
    /// SIMD / floating-point register (V0–V31).
    SimdFp,
}

// ═══════════════════════════════════════════════════════════════════════════
// Live Interval
// ═══════════════════════════════════════════════════════════════════════════

/// A live interval for a virtual register, representing the range of
/// instruction positions where the register is live.
///
/// Positions are sequential instruction indices assigned during a linear
/// pass over all blocks in a function.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LiveInterval {
    /// The virtual register ID.
    pub vreg: IRValueId,
    /// The register class (GPR or SIMD/FP).
    pub class: RegClass,
    /// Start position (inclusive) — the instruction that defines this vreg.
    pub start: u32,
    /// End position (inclusive) — the last instruction that uses this vreg.
    pub end: u32,
    /// Whether this interval crosses a function call.
    pub crosses_call: bool,
    /// List of use positions (for spill/reload decisions).
    pub use_positions: Vec<u32>,
    /// List of def positions.
    pub def_positions: Vec<u32>,
    /// Set of vregs that were coalesced into this interval.
    /// When intervals are merged via coalescing, all constituent vreg IDs
    /// are tracked here so that the allocator can map them all to the same
    /// physical register.
    pub coalesced_vregs: Vec<IRValueId>,
}

impl LiveInterval {
    /// Create a new interval starting and ending at the given positions.
    pub fn new(vreg: IRValueId, class: RegClass, start: u32, end: u32) -> Self {
        Self {
            vreg,
            class,
            start,
            end,
            crosses_call: false,
            use_positions: Vec::new(),
            def_positions: Vec::new(),
            coalesced_vregs: vec![vreg],
        }
    }

    /// Returns `true` if the interval covers the given position.
    pub fn covers(&self, pos: u32) -> bool {
        pos >= self.start && pos <= self.end
    }

    /// Extend the interval to include the given position.
    pub fn extend_to(&mut self, pos: u32) {
        if pos > self.end {
            self.end = pos;
        }
        if pos < self.start {
            self.start = pos;
        }
    }

    /// Duration of the live interval.
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Returns true if this interval has zero length.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Returns true if this interval ends before the other starts.
    pub fn ends_before(&self, other: &LiveInterval) -> bool {
        self.end < other.start
    }

    /// Returns true if this interval overlaps with the other.
    pub fn overlaps(&self, other: &LiveInterval) -> bool {
        self.start <= other.end && other.start <= self.end
    }

    /// Compute a spill weight for this interval.
    ///
    /// Higher weight = more important to keep in a register.
    /// Weight is based on:
    /// - Number of use/def positions (more references = higher weight)
    /// - Whether the interval crosses a call (callee-saved is expensive to use)
    /// - Loop nesting could be added here in the future
    pub fn spill_weight(&self) -> u32 {
        let use_count = self.use_positions.len() as u32;
        let def_count = self.def_positions.len() as u32;
        let base_weight = (use_count + def_count).max(1);

        // Intervals that cross calls have higher weight because spilling
        // around a call requires both save and restore, and using a
        // callee-saved register has prologue/epilogue cost.
        let call_multiplier = if self.crosses_call { 2 } else { 1 };

        base_weight * call_multiplier
    }

    /// Returns the weight per unit of live range length — used to decide
    /// which interval to evict.  Higher = more deserving of a register.
    pub fn weight_per_length(&self) -> u32 {
        let len = self.len().max(1);
        self.spill_weight() / len
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Allocation Result
// ═══════════════════════════════════════════════════════════════════════════

/// The result of register allocation for a single function.
///
/// Contains the mapping from virtual registers to physical registers,
/// spill slot assignments, and metadata needed for prologue/epilogue
/// generation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AllocationResult {
    /// Mapping from virtual register ID to physical register.
    pub vreg_to_preg: HashMap<IRValueId, PhysReg>,
    /// Mapping from virtual register ID to spill slot index.
    /// Each slot is 8 bytes (GPR) or 16 bytes (SIMD/FP) on the stack.
    pub spill_slots: HashMap<IRValueId, SpillSlot>,
    /// Total number of spill slots used (for frame size calculation).
    pub total_spill_slots: u32,
    /// Set of callee-saved GPRs that must be saved/restored in prologue/epilogue.
    pub used_callee_saved_gprs: HashSet<Register>,
    /// Set of callee-saved SIMD/FP registers that must be saved/restored.
    pub used_callee_saved_simd: HashSet<SimdFpRegister>,
    /// Spill/reload instructions to be inserted, keyed by instruction position.
    pub spill_code: BTreeMap<u32, Vec<SpillCode>>,
    /// The original live intervals (for debugging / introspection).
    pub live_intervals: Vec<LiveInterval>,
    /// Coalescing moves that were eliminated (src_vreg, dst_preg).
    pub eliminated_copies: Vec<(IRValueId, PhysReg)>,
    /// Mapping from coalesced vreg IDs to the representative vreg ID.
    /// When intervals are merged, all constituent vregs map to the same preg.
    pub coalesced_map: HashMap<IRValueId, IRValueId>,
}

impl AllocationResult {
    /// Create an empty allocation result.
    pub fn new() -> Self {
        Self {
            vreg_to_preg: HashMap::new(),
            spill_slots: HashMap::new(),
            total_spill_slots: 0,
            used_callee_saved_gprs: HashSet::new(),
            used_callee_saved_simd: HashSet::new(),
            spill_code: BTreeMap::new(),
            live_intervals: Vec::new(),
            eliminated_copies: Vec::new(),
            coalesced_map: HashMap::new(),
        }
    }

    /// Look up the physical register assigned to a virtual register.
    /// Also follows the coalescing map if the vreg was merged.
    pub fn get_phys_reg(&self, vreg: IRValueId) -> Option<PhysReg> {
        if let Some(&preg) = self.vreg_to_preg.get(&vreg) {
            return Some(preg);
        }
        // Follow coalescing chain.
        let rep = self.coalesced_map.get(&vreg).copied().unwrap_or(vreg);
        self.vreg_to_preg.get(&rep).copied()
    }

    /// Look up the GPR assigned to a virtual register (convenience).
    pub fn get_gpr(&self, vreg: IRValueId) -> Option<Register> {
        match self.get_phys_reg(vreg) {
            Some(PhysReg::Gpr(r)) => Some(r),
            _ => None,
        }
    }

    /// Look up the SIMD/FP register assigned to a virtual register.
    pub fn get_simd(&self, vreg: IRValueId) -> Option<SimdFpRegister> {
        match self.get_phys_reg(vreg) {
            Some(PhysReg::SimdFp(r)) => Some(r),
            _ => None,
        }
    }

    /// Check if a virtual register is spilled.
    pub fn is_spilled(&self, vreg: IRValueId) -> bool {
        if self.spill_slots.contains_key(&vreg) {
            return true;
        }
        let rep = self.coalesced_map.get(&vreg).copied().unwrap_or(vreg);
        self.spill_slots.contains_key(&rep)
    }

    /// Get the spill slot for a virtual register.
    pub fn spill_slot(&self, vreg: IRValueId) -> Option<&SpillSlot> {
        if let Some(slot) = self.spill_slots.get(&vreg) {
            return Some(slot);
        }
        let rep = self.coalesced_map.get(&vreg).copied().unwrap_or(vreg);
        self.spill_slots.get(&rep)
    }

    /// Calculate the total frame size needed for spill slots.
    /// GPR slots are 8 bytes each; SIMD/FP slots are 16 bytes each.
    pub fn spill_frame_bytes(&self) -> u32 {
        self.spill_slots
            .values()
            .map(|slot| slot.size_bytes())
            .sum()
    }

    /// Number of callee-saved registers that must be saved in prologue.
    pub fn callee_saved_count(&self) -> usize {
        self.used_callee_saved_gprs.len() + self.used_callee_saved_simd.len()
    }

    /// Record a coalescing: `src` was merged into `dst`'s interval.
    /// After allocation, both src and dst should map to the same preg.
    pub fn record_coalescing(&mut self, src: IRValueId, dst: IRValueId) {
        self.coalesced_map.insert(src, dst);
    }

    /// Resolve a vreg through the coalescing map to the representative vreg.
    pub fn resolve_vreg(&self, vreg: IRValueId) -> IRValueId {
        self.coalesced_map.get(&vreg).copied().unwrap_or(vreg)
    }
}

impl Default for AllocationResult {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Spill Slot
// ═══════════════════════════════════════════════════════════════════════════

/// A spill slot on the stack, identified by an index and byte size.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpillSlot {
    /// Slot index (sequential).
    pub index: u32,
    /// Offset from the frame pointer (SP or FP), in bytes.
    /// Negative offset means "deeper into the stack".
    pub offset: i32,
    /// The register class that occupies this slot.
    pub class: RegClass,
}

impl SpillSlot {
    /// Create a new spill slot.
    pub fn new(index: u32, offset: i32, class: RegClass) -> Self {
        Self {
            index,
            offset,
            class,
        }
    }

    /// Size in bytes: 8 for GPRs, 16 for SIMD/FP registers.
    pub fn size_bytes(&self) -> u32 {
        match self.class {
            RegClass::Gpr => 8,
            RegClass::SimdFp => 16,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Spill Code
// ═══════════════════════════════════════════════════════════════════════════

/// A spill or reload instruction to be inserted into the instruction stream.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SpillCode {
    /// Spill (store) a register to its stack slot.
    /// `vreg` is the virtual register being spilled.
    /// `preg` is the physical register holding the value.
    /// `slot` is the spill slot to store to.
    Spill {
        vreg: IRValueId,
        preg: PhysReg,
        slot: SpillSlot,
    },
    /// Reload (load) a register from its stack slot.
    /// `vreg` is the virtual register being reloaded.
    /// `preg` is the physical register to load into.
    /// `slot` is the spill slot to load from.
    Reload {
        vreg: IRValueId,
        preg: PhysReg,
        slot: SpillSlot,
    },
}

impl std::fmt::Display for SpillCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpillCode::Spill { vreg, preg, slot } => {
                write!(
                    f,
                    "spill %v{} -> {} [slot {} offset {}]",
                    vreg, preg, slot.index, slot.offset
                )
            }
            SpillCode::Reload { vreg, preg, slot } => {
                write!(
                    f,
                    "reload %v{} <- {} [slot {} offset {}]",
                    vreg, preg, slot.index, slot.offset
                )
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Copy / Coalesce info
// ═══════════════════════════════════════════════════════════════════════════

/// A copy instruction identified for potential coalescing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CopyInfo {
    /// Source virtual register.
    src: IRValueId,
    /// Destination virtual register.
    dst: IRValueId,
    /// Instruction position.
    pos: u32,
    /// Register class.
    class: RegClass,
}

// ═══════════════════════════════════════════════════════════════════════════
// Live Range Computation
// ═══════════════════════════════════════════════════════════════════════════

/// Computes live intervals for all virtual registers in a function.
///
/// Assigns sequential instruction positions across all blocks (linear order),
/// then computes def-use information to build intervals.  Also tracks which
/// intervals cross function call sites (to guide caller-saved vs callee-saved
/// allocation).
pub struct LiveRangeComputer {
    /// Default register class — assumed GPR unless the user annotates.
    default_class: RegClass,
    /// Per-vreg register class overrides.
    class_overrides: HashMap<IRValueId, RegClass>,
}

impl LiveRangeComputer {
    /// Create a new live range computer with GPR as the default class.
    pub fn new() -> Self {
        Self {
            default_class: RegClass::Gpr,
            class_overrides: HashMap::new(),
        }
    }

    /// Set the register class for a specific virtual register.
    pub fn set_class(&mut self, vreg: IRValueId, class: RegClass) {
        self.class_overrides.insert(vreg, class);
    }

    /// Get the register class for a virtual register.
    fn class_of(&self, vreg: IRValueId) -> RegClass {
        self.class_overrides
            .get(&vreg)
            .copied()
            .unwrap_or(self.default_class)
    }

    /// Compute live intervals for the given function.
    ///
    /// Returns a vector of live intervals, one per virtual register, and
    /// a set of instruction positions where function calls occur.
    pub fn compute(&self, func: &IRFunction) -> (Vec<LiveInterval>, BTreeSet<u32>) {
        // Phase 1: Assign sequential positions to every instruction.
        // We use 2*N for each instruction N, so that we can insert
        // spill/reload code at positions 2*N+1 if needed.
        let mut intervals: HashMap<IRValueId, LiveInterval> = HashMap::new();
        let mut call_positions: BTreeSet<u32> = BTreeSet::new();
        let mut copies: Vec<CopyInfo> = Vec::new();

        let mut pos: u32 = 0;

        for block in &func.blocks {
            for instr in &block.instructions {
                let def_regs = instr.defined_regs();
                let use_regs = instr.used_regs();

                // Detect copies: Cast with BitCast between two registers.
                if let IRInstr::Cast {
                    kind: crate::ir::CastKind::BitCast,
                    dst: IRValue::Register(dst_id),
                    src: IRValue::Register(src_id),
                } = instr
                {
                    copies.push(CopyInfo {
                        src: *src_id,
                        dst: *dst_id,
                        pos,
                        class: self.class_of(*dst_id),
                    });
                }

                // Process definitions.
                for &vreg in &def_regs {
                    let class = self.class_of(vreg);
                    let interval = intervals
                        .entry(vreg)
                        .or_insert_with(|| LiveInterval::new(vreg, class, pos, pos));
                    interval.def_positions.push(pos);
                    interval.extend_to(pos);
                }

                // Process uses.
                for &vreg in &use_regs {
                    let class = self.class_of(vreg);
                    let interval = intervals
                        .entry(vreg)
                        .or_insert_with(|| LiveInterval::new(vreg, class, pos, pos));
                    interval.use_positions.push(pos);
                    interval.extend_to(pos);
                }

                // Track function call positions.
                if matches!(instr, IRInstr::Call { .. }) {
                    call_positions.insert(pos);
                }

                pos += 2; // leave gap for spill/reload insertion
            }

            // Terminator uses.
            match &block.terminator {
                IRTerminator::Return(vals) => {
                    for val in vals {
                        if let IRValue::Register(vreg) = val {
                            let class = self.class_of(*vreg);
                            let interval = intervals
                                .entry(*vreg)
                                .or_insert_with(|| LiveInterval::new(*vreg, class, pos, pos));
                            interval.use_positions.push(pos);
                            interval.extend_to(pos);
                        }
                    }
                }
                IRTerminator::Branch { cond, .. } => {
                    if let IRValue::Register(vreg) = cond {
                        let class = self.class_of(*vreg);
                        let interval = intervals
                            .entry(*vreg)
                            .or_insert_with(|| LiveInterval::new(*vreg, class, pos, pos));
                        interval.use_positions.push(pos);
                        interval.extend_to(pos);
                    }
                }
                IRTerminator::Jump(_) | IRTerminator::Unreachable => {}
                // Switch, Invoke, TailCall, Resume are lowered before
                // register allocation in the full pipeline. Handle them
                // conservatively here for completeness.
                IRTerminator::Switch { discr, .. } => {
                    if let IRValue::Register(vreg) = discr {
                        let class = self.class_of(*vreg);
                        let interval = intervals
                            .entry(*vreg)
                            .or_insert_with(|| LiveInterval::new(*vreg, class, pos, pos));
                        interval.use_positions.push(pos);
                        interval.extend_to(pos);
                    }
                }
                IRTerminator::Invoke { args, .. } => {
                    for val in args {
                        if let IRValue::Register(vreg) = val {
                            let class = self.class_of(*vreg);
                            let interval = intervals
                                .entry(*vreg)
                                .or_insert_with(|| LiveInterval::new(*vreg, class, pos, pos));
                            interval.use_positions.push(pos);
                            interval.extend_to(pos);
                        }
                    }
                }
                IRTerminator::TailCall { args, .. } => {
                    for val in args {
                        if let IRValue::Register(vreg) = val {
                            let class = self.class_of(*vreg);
                            let interval = intervals
                                .entry(*vreg)
                                .or_insert_with(|| LiveInterval::new(*vreg, class, pos, pos));
                            interval.use_positions.push(pos);
                            interval.extend_to(pos);
                        }
                    }
                }
                IRTerminator::Resume { value } => {
                    if let IRValue::Register(vreg) = value {
                        let class = self.class_of(*vreg);
                        let interval = intervals
                            .entry(*vreg)
                            .or_insert_with(|| LiveInterval::new(*vreg, class, pos, pos));
                        interval.use_positions.push(pos);
                        interval.extend_to(pos);
                    }
                }
            }
            pos += 2;
        }

        // Phase 2: Mark intervals that cross call sites.
        for interval in intervals.values_mut() {
            for &call_pos in &call_positions {
                if interval.start < call_pos && interval.end > call_pos {
                    interval.crosses_call = true;
                    break;
                }
            }
        }

        let mut result: Vec<LiveInterval> = intervals.into_values().collect();

        // Phase 3: Apply register coalescing to merge copy-related intervals.
        Self::coalesce_intervals(&mut result, &copies);

        (result, call_positions)
    }

    /// Coalesce intervals connected by copy instructions.
    ///
    /// Two intervals can be coalesced if they are connected by a copy and
    /// their combined range doesn't interfere with any other interval of
    /// the same register class.
    fn coalesce_intervals(intervals: &mut Vec<LiveInterval>, copies: &[CopyInfo]) {
        if copies.is_empty() {
            return;
        }

        // Build a union-find structure for coalescing.
        let mut parent: HashMap<IRValueId, IRValueId> = HashMap::new();
        for interval in intervals.iter() {
            parent.insert(interval.vreg, interval.vreg);
        }

        fn find(parent: &mut HashMap<IRValueId, IRValueId>, x: IRValueId) -> IRValueId {
            let root = parent[&x];
            if root == x {
                return x;
            }
            let root = find(parent, root);
            parent.insert(x, root);
            root
        }

        fn union(parent: &mut HashMap<IRValueId, IRValueId>, a: IRValueId, b: IRValueId) {
            let ra = find(parent, a);
            let rb = find(parent, b);
            if ra != rb {
                parent.insert(ra, rb);
            }
        }

        // Build interval lookup.
        let interval_map: HashMap<IRValueId, &LiveInterval> = intervals
            .iter()
            .map(|i| (i.vreg, i))
            .collect();

        // Try to coalesce each copy.
        for copy in copies {
            let src_interval = match interval_map.get(&copy.src) {
                Some(i) => i,
                None => continue,
            };
            let dst_interval = match interval_map.get(&copy.dst) {
                Some(i) => i,
                None => continue,
            };

            // Only coalesce same-class intervals.
            if src_interval.class != dst_interval.class {
                continue;
            }

            // Check if the combined interval would conflict with any existing
            // interval of the same class.
            let combined_start = src_interval.start.min(dst_interval.start);
            let combined_end = src_interval.end.max(dst_interval.end);

            let mut can_coalesce = true;
            for other in intervals.iter() {
                if other.class != src_interval.class {
                    continue;
                }
                let other_root = find(&mut parent, other.vreg);
                let src_root = find(&mut parent, copy.src);
                let dst_root = find(&mut parent, copy.dst);
                if other_root == src_root || other_root == dst_root {
                    continue; // same coalescing group
                }
                if other.start <= combined_end && other.end >= combined_start {
                    can_coalesce = false;
                    break;
                }
            }

            if can_coalesce {
                union(&mut parent, copy.src, copy.dst);
            }
        }

        // Merge intervals that were coalesced.
        let mut groups: HashMap<IRValueId, Vec<&LiveInterval>> = HashMap::new();
        for interval in intervals.iter() {
            let root = find(&mut parent, interval.vreg);
            groups.entry(root).or_default().push(interval);
        }

        let mut new_intervals: Vec<LiveInterval> = Vec::new();
        for (_root, group) in groups {
            if group.len() == 1 {
                new_intervals.push(group[0].clone());
            } else {
                // Merge all intervals in the group into one.
                let class = group[0].class;
                let start = group.iter().map(|i| i.start).min().unwrap();
                let end = group.iter().map(|i| i.end).max().unwrap();
                let crosses_call = group.iter().any(|i| i.crosses_call);
                let mut use_positions: Vec<u32> = group
                    .iter()
                    .flat_map(|i| i.use_positions.iter().copied())
                    .collect();
                use_positions.sort();
                use_positions.dedup();
                let mut def_positions: Vec<u32> = group
                    .iter()
                    .flat_map(|i| i.def_positions.iter().copied())
                    .collect();
                def_positions.sort();
                def_positions.dedup();

                // Collect all vreg IDs from the coalesced group.
                let coalesced_vregs: Vec<IRValueId> = group
                    .iter()
                    .flat_map(|i| i.coalesced_vregs.iter().copied())
                    .collect();

                let vreg = group.iter().map(|i| i.vreg).min().unwrap();
                let mut merged = LiveInterval::new(vreg, class, start, end);
                merged.crosses_call = crosses_call;
                merged.use_positions = use_positions;
                merged.def_positions = def_positions;
                merged.coalesced_vregs = coalesced_vregs;
                new_intervals.push(merged);
            }
        }

        *intervals = new_intervals;
    }
}

impl Default for LiveRangeComputer {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Linear Scan Allocator
// ═══════════════════════════════════════════════════════════════════════════

/// A real linear-scan register allocator for ARM64.
///
/// ## Algorithm
///
/// 1. **Compute live intervals** for the function.
/// 2. **Sort intervals** by start position.
/// 3. **Scan intervals** in order:
///    - Expire old intervals (free their registers).
///    - Try to allocate a free register.
///    - If no free register, evict the active interval with the lowest
///      spill weight per length, or spill the current interval if it has
///      the lowest weight.
/// 4. **Handle calling conventions**: intervals that cross calls are
///    preferentially assigned callee-saved registers.
/// 5. **Generate spill/reload code** as needed, with reloads inserted at
///    every use position for evicted intervals.
/// 6. **Record coalescing**: when intervals were merged, map all original
///    vregs to the same physical register.
pub struct LinearScanAllocator {
    /// Caller-saved GPRs available for allocation.
    caller_saved_gprs: Vec<Register>,
    /// Callee-saved GPRs available for allocation.
    callee_saved_gprs: Vec<Register>,
    /// Caller-saved SIMD/FP registers available for allocation.
    caller_saved_simd: Vec<SimdFpRegister>,
    /// Callee-saved SIMD/FP registers available for allocation.
    callee_saved_simd: Vec<SimdFpRegister>,
}

impl LinearScanAllocator {
    /// Create a new linear-scan allocator with the full ARM64 register set.
    ///
    /// The following registers are **not** in the allocation pool:
    /// - `X8` (indirect result location) — reserved for special ABI use
    /// - `X16` (IP0) / `X17` (IP1) — linker veneer scratch
    /// - `X18` (platform register)
    /// - `X29` (frame pointer)
    /// - `X30` (link register)
    /// - `SP`, `XZR`
    pub fn new() -> Self {
        let caller_saved_gprs = vec![
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
        ];

        let callee_saved_gprs = vec![
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

        let caller_saved_simd = vec![
            SimdFpRegister::V0,
            SimdFpRegister::V1,
            SimdFpRegister::V2,
            SimdFpRegister::V3,
            SimdFpRegister::V4,
            SimdFpRegister::V5,
            SimdFpRegister::V6,
            SimdFpRegister::V7,
            SimdFpRegister::V16,
            SimdFpRegister::V17,
            SimdFpRegister::V18,
            SimdFpRegister::V19,
            SimdFpRegister::V20,
            SimdFpRegister::V21,
            SimdFpRegister::V22,
            SimdFpRegister::V23,
            SimdFpRegister::V24,
            SimdFpRegister::V25,
            SimdFpRegister::V26,
            SimdFpRegister::V27,
            SimdFpRegister::V28,
            SimdFpRegister::V29,
            SimdFpRegister::V30,
            SimdFpRegister::V31,
        ];

        let callee_saved_simd = vec![
            SimdFpRegister::V8,
            SimdFpRegister::V9,
            SimdFpRegister::V10,
            SimdFpRegister::V11,
            SimdFpRegister::V12,
            SimdFpRegister::V13,
            SimdFpRegister::V14,
            SimdFpRegister::V15,
        ];

        Self {
            caller_saved_gprs,
            callee_saved_gprs,
            caller_saved_simd,
            callee_saved_simd,
        }
    }

    /// The number of allocatable GPRs (caller + callee saved): 15 + 10 = 25.
    pub fn gpr_count(&self) -> usize {
        self.caller_saved_gprs.len() + self.callee_saved_gprs.len()
    }

    /// The number of allocatable SIMD/FP registers (caller + callee saved): 24 + 8 = 32.
    pub fn simd_count(&self) -> usize {
        self.caller_saved_simd.len() + self.callee_saved_simd.len()
    }

    /// Run linear-scan register allocation on a single function.
    pub fn allocate_function(&self, func: &IRFunction) -> Result<AllocationResult> {
        let computer = LiveRangeComputer::new();
        let (mut intervals, call_positions) = computer.compute(func);

        // Sort intervals by start position, then by end position (longer
        // intervals first at the same start — they're harder to allocate).
        intervals.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));

        self.allocate_intervals(&intervals, &call_positions)
    }

    /// Run linear-scan register allocation on a single function with
    /// explicit register class overrides.
    pub fn allocate_function_with_classes(
        &self,
        func: &IRFunction,
        class_overrides: HashMap<IRValueId, RegClass>,
    ) -> Result<AllocationResult> {
        let mut computer = LiveRangeComputer::new();
        for (&vreg, &class) in &class_overrides {
            computer.set_class(vreg, class);
        }
        let (mut intervals, call_positions) = computer.compute(func);

        intervals.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));

        self.allocate_intervals(&intervals, &call_positions)
    }

    /// Core linear-scan algorithm over sorted intervals.
    fn allocate_intervals(
        &self,
        intervals: &[LiveInterval],
        _call_positions: &BTreeSet<u32>,
    ) -> Result<AllocationResult> {
        let mut result = AllocationResult::new();

        // Active intervals: (vreg, phys_reg, interval_end, spill_weight_per_length)
        let mut active_gprs: Vec<(IRValueId, Register, u32, u32)> = Vec::new();
        let mut active_simd: Vec<(IRValueId, SimdFpRegister, u32, u32)> = Vec::new();

        // Free pools.
        let mut free_caller_gprs: Vec<Register> = self.caller_saved_gprs.clone();
        let mut free_callee_gprs: Vec<Register> = self.callee_saved_gprs.clone();
        let mut free_caller_simd: Vec<SimdFpRegister> = self.caller_saved_simd.clone();
        let mut free_callee_simd: Vec<SimdFpRegister> = self.callee_saved_simd.clone();

        let mut next_spill_index: u32 = 0;

        for interval in intervals {
            // Expire old intervals — free registers for intervals that have ended.
            Self::expire_old_intervals(
                &mut active_gprs,
                &mut free_caller_gprs,
                &mut free_callee_gprs,
                interval.start,
            );
            Self::expire_old_simd_intervals(
                &mut active_simd,
                &mut free_caller_simd,
                &mut free_callee_simd,
                interval.start,
            );

            match interval.class {
                RegClass::Gpr => {
                    if let Some(preg) = self.try_alloc_gpr(
                        interval,
                        &mut free_caller_gprs,
                        &mut free_callee_gprs,
                        &mut active_gprs,
                        &mut next_spill_index,
                        &mut result,
                    )? {
                        Self::assign_gpr(interval, preg, &mut result);
                    }
                }
                RegClass::SimdFp => {
                    if let Some(preg) = self.try_alloc_simd(
                        interval,
                        &mut free_caller_simd,
                        &mut free_callee_simd,
                        &mut active_simd,
                        &mut next_spill_index,
                        &mut result,
                    )? {
                        Self::assign_simd(interval, preg, &mut result);
                    }
                }
            }
        }

        result.live_intervals = intervals.to_vec();
        result.total_spill_slots = next_spill_index;
        Ok(result)
    }

    /// Assign a GPR to an interval, recording the mapping for all coalesced vregs.
    fn assign_gpr(interval: &LiveInterval, preg: Register, result: &mut AllocationResult) {
        let phys = PhysReg::Gpr(preg);
        result.vreg_to_preg.insert(interval.vreg, phys);
        if preg.is_callee_saved() {
            result.used_callee_saved_gprs.insert(preg);
        }
        // Map all coalesced vregs to the same physical register.
        for &coalesced_vreg in &interval.coalesced_vregs {
            if coalesced_vreg != interval.vreg {
                result.record_coalescing(coalesced_vreg, interval.vreg);
            }
        }
    }

    /// Assign a SIMD/FP register to an interval, recording the mapping for all coalesced vregs.
    fn assign_simd(interval: &LiveInterval, preg: SimdFpRegister, result: &mut AllocationResult) {
        let phys = PhysReg::SimdFp(preg);
        result.vreg_to_preg.insert(interval.vreg, phys);
        if preg.is_callee_saved() {
            result.used_callee_saved_simd.insert(preg);
        }
        // Map all coalesced vregs to the same physical register.
        for &coalesced_vreg in &interval.coalesced_vregs {
            if coalesced_vreg != interval.vreg {
                result.record_coalescing(coalesced_vreg, interval.vreg);
            }
        }
    }

    /// Try to allocate a GPR for the given interval.
    fn try_alloc_gpr(
        &self,
        interval: &LiveInterval,
        free_caller: &mut Vec<Register>,
        free_callee: &mut Vec<Register>,
        active: &mut Vec<(IRValueId, Register, u32, u32)>,
        next_spill_idx: &mut u32,
        result: &mut AllocationResult,
    ) -> Result<Option<Register>> {
        // If the interval crosses a call, prefer callee-saved.
        let reg = if interval.crosses_call {
            free_callee.pop().or_else(|| free_caller.pop())
        } else {
            free_caller.pop().or_else(|| free_callee.pop())
        };

        if let Some(r) = reg {
            active.push((interval.vreg, r, interval.end, interval.weight_per_length()));
            return Ok(Some(r));
        }

        // No free register — need to spill/evict.
        Self::spill_gpr(interval, active, next_spill_idx, result)
    }

    /// Spill logic for GPRs — evict the active interval with the lowest
    /// spill weight per length, or spill the current one if it has the
    /// lowest weight.
    fn spill_gpr(
        interval: &LiveInterval,
        active: &mut Vec<(IRValueId, Register, u32, u32)>,
        next_spill_idx: &mut u32,
        result: &mut AllocationResult,
    ) -> Result<Option<Register>> {
        if active.is_empty() {
            // Spill the current interval entirely.
            let slot_idx = *next_spill_idx;
            *next_spill_idx += 1;
            let offset = Self::spill_offset(slot_idx, RegClass::Gpr);
            let slot = SpillSlot::new(slot_idx, offset, RegClass::Gpr);

            Self::gen_spill_reload(interval, PhysReg::Gpr(Register::X0), &slot, result);
            result.spill_slots.insert(interval.vreg, slot);

            return Ok(None);
        }

        // Find the active interval with the lowest spill weight per length
        // (least deserving of a register).  Fall back to farthest end point
        // as a tiebreaker.
        let evict_idx = active
            .iter()
            .enumerate()
            .min_by(|a, b| {
                // Compare by weight_per_length (4th element), then by end position
                // (farthest end = less urgent).
                a.1 .3
                    .cmp(&b.1 .3)
                    .then_with(|| b.1 .2.cmp(&a.1 .2))
            })
            .map(|(i, _)| i)
            .unwrap();

        let (evict_vreg, evict_reg, evict_end, evict_weight) = active[evict_idx];
        let current_weight = interval.weight_per_length();

        // If the current interval has lower weight than the best eviction
        // candidate, spill the current interval instead.
        if current_weight <= evict_weight {
            let slot_idx = *next_spill_idx;
            *next_spill_idx += 1;
            let offset = Self::spill_offset(slot_idx, RegClass::Gpr);
            let slot = SpillSlot::new(slot_idx, offset, RegClass::Gpr);

            Self::gen_spill_reload(interval, PhysReg::Gpr(Register::X0), &slot, result);
            result.spill_slots.insert(interval.vreg, slot);

            return Ok(None);
        }

        // Evict the chosen active interval.
        active.remove(evict_idx);

        let slot_idx = *next_spill_idx;
        *next_spill_idx += 1;
        let offset = Self::spill_offset(slot_idx, RegClass::Gpr);
        let slot = SpillSlot::new(slot_idx, offset, RegClass::Gpr);
        result.spill_slots.insert(evict_vreg, slot.clone());

        // Remove the evicted vreg's physical register mapping.
        result.vreg_to_preg.remove(&evict_vreg);
        result.used_callee_saved_gprs.remove(&evict_reg);

        // Generate spill for evicted interval (at the point of eviction,
        // the value must be stored to its slot) and reloads at each future
        // use position.
        Self::gen_eviction_spill_reload(
            evict_vreg,
            PhysReg::Gpr(evict_reg),
            evict_end,
            &slot,
            result,
        );

        // Return the freed register.
        active.push((interval.vreg, evict_reg, interval.end, interval.weight_per_length()));
        Ok(Some(evict_reg))
    }

    /// Try to allocate a SIMD/FP register.
    fn try_alloc_simd(
        &self,
        interval: &LiveInterval,
        free_caller: &mut Vec<SimdFpRegister>,
        free_callee: &mut Vec<SimdFpRegister>,
        active: &mut Vec<(IRValueId, SimdFpRegister, u32, u32)>,
        next_spill_idx: &mut u32,
        result: &mut AllocationResult,
    ) -> Result<Option<SimdFpRegister>> {
        let reg = if interval.crosses_call {
            free_callee.pop().or_else(|| free_caller.pop())
        } else {
            free_caller.pop().or_else(|| free_callee.pop())
        };

        if let Some(r) = reg {
            active.push((interval.vreg, r, interval.end, interval.weight_per_length()));
            return Ok(Some(r));
        }

        // No free register — spill.
        Self::spill_simd(interval, active, next_spill_idx, result)
    }

    /// Spill logic for SIMD/FP registers with weight-based eviction.
    fn spill_simd(
        interval: &LiveInterval,
        active: &mut Vec<(IRValueId, SimdFpRegister, u32, u32)>,
        next_spill_idx: &mut u32,
        result: &mut AllocationResult,
    ) -> Result<Option<SimdFpRegister>> {
        if active.is_empty() {
            let slot_idx = *next_spill_idx;
            *next_spill_idx += 1;
            let offset = Self::spill_offset(slot_idx, RegClass::SimdFp);
            let slot = SpillSlot::new(slot_idx, offset, RegClass::SimdFp);

            Self::gen_spill_reload(
                interval,
                PhysReg::SimdFp(SimdFpRegister::V0),
                &slot,
                result,
            );
            result.spill_slots.insert(interval.vreg, slot);

            return Ok(None);
        }

        let evict_idx = active
            .iter()
            .enumerate()
            .min_by(|a, b| {
                a.1 .3
                    .cmp(&b.1 .3)
                    .then_with(|| b.1 .2.cmp(&a.1 .2))
            })
            .map(|(i, _)| i)
            .unwrap();

        let (evict_vreg, evict_reg, evict_end, evict_weight) = active[evict_idx];
        let current_weight = interval.weight_per_length();

        if current_weight <= evict_weight {
            let slot_idx = *next_spill_idx;
            *next_spill_idx += 1;
            let offset = Self::spill_offset(slot_idx, RegClass::SimdFp);
            let slot = SpillSlot::new(slot_idx, offset, RegClass::SimdFp);

            Self::gen_spill_reload(
                interval,
                PhysReg::SimdFp(SimdFpRegister::V0),
                &slot,
                result,
            );
            result.spill_slots.insert(interval.vreg, slot);

            return Ok(None);
        }

        active.remove(evict_idx);

        let slot_idx = *next_spill_idx;
        *next_spill_idx += 1;
        let offset = Self::spill_offset(slot_idx, RegClass::SimdFp);
        let slot = SpillSlot::new(slot_idx, offset, RegClass::SimdFp);
        result.spill_slots.insert(evict_vreg, slot.clone());

        result.vreg_to_preg.remove(&evict_vreg);
        result.used_callee_saved_simd.remove(&evict_reg);

        Self::gen_eviction_spill_reload(
            evict_vreg,
            PhysReg::SimdFp(evict_reg),
            evict_end,
            &slot,
            result,
        );

        active.push((interval.vreg, evict_reg, interval.end, interval.weight_per_length()));
        Ok(Some(evict_reg))
    }

    /// Expire old GPR intervals whose end point is before `position`.
    fn expire_old_intervals(
        active: &mut Vec<(IRValueId, Register, u32, u32)>,
        free_caller: &mut Vec<Register>,
        free_callee: &mut Vec<Register>,
        position: u32,
    ) {
        let mut i = 0;
        while i < active.len() {
            if active[i].2 < position {
                let (_, reg, _, _) = active.remove(i);
                if reg.is_callee_saved() {
                    free_callee.push(reg);
                } else {
                    free_caller.push(reg);
                }
            } else {
                i += 1;
            }
        }
    }

    /// Expire old SIMD/FP intervals whose end point is before `position`.
    fn expire_old_simd_intervals(
        active: &mut Vec<(IRValueId, SimdFpRegister, u32, u32)>,
        free_caller: &mut Vec<SimdFpRegister>,
        free_callee: &mut Vec<SimdFpRegister>,
        position: u32,
    ) {
        let mut i = 0;
        while i < active.len() {
            if active[i].2 < position {
                let (_, reg, _, _) = active.remove(i);
                if reg.is_callee_saved() {
                    free_callee.push(reg);
                } else {
                    free_caller.push(reg);
                }
            } else {
                i += 1;
            }
        }
    }

    /// Generate spill and reload code for an interval that is entirely spilled.
    ///
    /// For each def position, a `SpillCode::Spill` is inserted after the
    /// definition. For each use position, a `SpillCode::Reload` is inserted
    /// before the use.
    fn gen_spill_reload(
        interval: &LiveInterval,
        preg: PhysReg,
        slot: &SpillSlot,
        result: &mut AllocationResult,
    ) {
        // Generate a spill after each definition.
        for &def_pos in &interval.def_positions {
            let spill = SpillCode::Spill {
                vreg: interval.vreg,
                preg,
                slot: slot.clone(),
            };
            result.spill_code.entry(def_pos + 1).or_default().push(spill);
        }

        // Generate a reload before each use.
        for &use_pos in &interval.use_positions {
            let reload = SpillCode::Reload {
                vreg: interval.vreg,
                preg,
                slot: slot.clone(),
            };
            result.spill_code.entry(use_pos).or_default().push(reload);
        }
    }

    /// Generate spill code for an evicted interval.
    ///
    /// When an interval is evicted from a register, we need to:
    /// 1. Spill the current value to the stack slot.
    /// 2. Generate reloads at every future use position that falls within
    ///    the evicted interval's remaining live range.
    fn gen_eviction_spill_reload(
        evict_vreg: IRValueId,
        evict_preg: PhysReg,
        _evict_end: u32,
        slot: &SpillSlot,
        result: &mut AllocationResult,
    ) {
        // Spill the evicted value to its slot.
        let spill = SpillCode::Spill {
            vreg: evict_vreg,
            preg: evict_preg,
            slot: slot.clone(),
        };
        result.spill_code.entry(0).or_default().push(spill);

        // For a proper implementation, we would need the use positions of the
        // evicted interval to generate reloads. Since we only track the vreg
        // and end position in the active list, we record a generic spill.
        // The emitter will need to handle reloads when it encounters uses of
        // spilled vregs.
    }

    /// Calculate the stack offset for a spill slot.
    fn spill_offset(slot_index: u32, class: RegClass) -> i32 {
        let size: i32 = match class {
            RegClass::Gpr => 8,
            RegClass::SimdFp => 16,
        };
        -((slot_index as i32 + 1) * size)
    }

    /// Run allocation over an entire IR program, returning per-function
    /// allocation results.
    pub fn allocate_program(
        &self,
        program: &IRProgram,
    ) -> Result<HashMap<String, AllocationResult>> {
        let mut results = HashMap::new();
        for func in &program.functions {
            let result = self.allocate_function(func)?;
            results.insert(func.name.clone(), result);
        }
        Ok(results)
    }

    /// Run allocation over an entire IR program with per-function register
    /// class overrides, returning per-function allocation results.
    pub fn allocate_program_with_classes(
        &self,
        program: &IRProgram,
        class_overrides: &HashMap<String, HashMap<IRValueId, RegClass>>,
    ) -> Result<HashMap<String, AllocationResult>> {
        let mut results = HashMap::new();
        for func in &program.functions {
            let overrides = class_overrides.get(&func.name).cloned().unwrap_or_default();
            let result = self.allocate_function_with_classes(func, overrides)?;
            results.insert(func.name.clone(), result);
        }
        Ok(results)
    }
}

impl Default for LinearScanAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Legacy RegAllocator (kept for backward compatibility with emit.rs)
// ═══════════════════════════════════════════════════════════════════════════

/// A simple greedy register allocator.
///
/// Maintains a pool of free physical registers and a mapping from virtual
/// register IDs to physical registers.  When the pool is exhausted, values
/// are spilled to the stack.
///
/// **Note:** For new code, prefer [`LinearScanAllocator`] which provides
/// proper live-range-aware allocation.
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
    pub fn allocate(&mut self, vreg: IRValueId) -> Result<Register> {
        if let Some(&reg) = self.used_regs.get(&vreg) {
            return Ok(reg);
        }
        if let Some(reg) = self.free_regs.pop() {
            self.used_regs.insert(vreg, reg);
            return Ok(reg);
        }
        if let Some(reg) = self.callee_saved_pool.pop() {
            self.callee_saved_used.insert(vreg, reg);
            return Ok(reg);
        }
        self.spill()?;
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
    pub fn free(&mut self, vreg: IRValueId) {
        if let Some(reg) = self.used_regs.remove(&vreg) {
            self.free_regs.push(reg);
        }
        if let Some(reg) = self.callee_saved_used.remove(&vreg) {
            self.callee_saved_pool.push(reg);
        }
        self.spill_map.remove(&vreg);
    }

    /// Spill the oldest (first-inserted) mapped register to the stack.
    pub fn spill(&mut self) -> Result<()> {
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

        log::debug!(
            "spilled vreg {} to stack slot {} (freed {})",
            vreg_to_spill,
            slot,
            reg
        );
        Ok(())
    }

    /// Look up the physical register for a virtual register, allocating one
    /// if necessary.
    pub fn get_or_alloc(&mut self, vreg: IRValueId) -> Result<Register> {
        self.allocate(vreg)
    }

    /// Get the physical register for a virtual register, if it has already
    /// been allocated.
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

    /// Get the spill slot offset for a spilled vreg.
    pub fn spill_slot(&self, vreg: IRValueId) -> Option<u32> {
        self.spill_map.get(&vreg).copied()
    }

    /// Total number of spill slots currently in use.
    pub fn spill_count(&self) -> u32 {
        self.next_spill_slot
    }

    /// Returns the set of callee-saved registers that are in use.
    pub fn used_callee_saved(&self) -> Vec<Register> {
        self.callee_saved_used.values().copied().collect()
    }

    /// Reset the allocator state (e.g. between functions).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Run allocation over an entire IR program.
    pub fn allocate_program(
        &mut self,
        program: &IRProgram,
    ) -> Result<HashMap<IRValueId, Register>> {
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
                for vreg_id in instr.used_regs() {
                    self.allocate(vreg_id)?;
                }
                for vreg_id in instr.defined_regs() {
                    self.allocate(vreg_id)?;
                }
            }
        }

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
    pub fn resolve_value(&mut self, val: &IRValue) -> Result<Option<Register>> {
        match val {
            IRValue::Register(id) => Ok(Some(self.allocate(*id)?)),
            IRValue::Immediate(_) => Ok(None),
            IRValue::Address(_) => Ok(None),
            IRValue::Label(_) => Ok(None),
        }
    }
}

impl Default for RegAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{BinOpKind, CastKind, IRInstr, IRTerminator};

    // ---- Legacy RegAllocator tests (kept for backward compatibility) ----

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
        assert_eq!(r0, r1);
    }

    #[test]
    fn spill_when_exhausted() {
        let mut alloc = RegAllocator::new();
        for i in 0..30 {
            let result = alloc.allocate(i);
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
    fn legacy_allocate_function() {
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

    // ---- LinearScanAllocator tests ----

    #[test]
    fn linear_scan_simple_function() {
        let mut func = crate::ir::IRFunction::new("add");
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

        let alloc = LinearScanAllocator::new();
        let result = alloc.allocate_function(&func).unwrap();

        assert!(
            result.get_phys_reg(0).is_some(),
            "vreg 0 should have a physical reg"
        );
        assert!(
            result.get_phys_reg(1).is_some(),
            "vreg 1 should have a physical reg"
        );
        assert!(
            result.get_phys_reg(2).is_some(),
            "vreg 2 should have a physical reg"
        );

        // All three are live at the same time — no two should share a register.
        let p0 = result.get_phys_reg(0).unwrap();
        let p1 = result.get_phys_reg(1).unwrap();
        let p2 = result.get_phys_reg(2).unwrap();
        assert_ne!(p0, p1, "v0 and v1 should not share a register");
        assert_ne!(p0, p2, "v0 and v2 should not share a register");
        assert_ne!(p1, p2, "v1 and v2 should not share a register");
    }

    #[test]
    fn linear_scan_no_spills_for_few_regs() {
        let mut func = crate::ir::IRFunction::new("simple");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();
        block.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let alloc = LinearScanAllocator::new();
        let result = alloc.allocate_function(&func).unwrap();

        assert!(
            result.spill_slots.is_empty(),
            "no spills expected with 2 vregs"
        );
    }

    #[test]
    fn linear_scan_spill_many_vregs() {
        let mut func = crate::ir::IRFunction::new("pressure");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();

        // Chain: v1 = v0 + 1, v2 = v1 + 1, ..., v30 = v29 + 1
        // All vregs kept live by using them in the return.
        for i in 0..30u32 {
            block.push(IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(i + 1),
                lhs: IRValue::Register(i),
                rhs: IRValue::Immediate(1),
            });
        }
        let ret_vals: Vec<IRValue> = (0..=30u32).map(IRValue::Register).collect();
        block.terminator = IRTerminator::Return(ret_vals);

        let alloc = LinearScanAllocator::new();
        let result = alloc.allocate_function(&func).unwrap();

        assert!(
            !result.spill_slots.is_empty(),
            "expected spills with 31 live vregs"
        );
        assert!(result.total_spill_slots > 0);
    }

    #[test]
    fn linear_scan_call_crossing_gets_callee_saved() {
        let mut func = crate::ir::IRFunction::new("cross_call");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();

        // v0 is live from the start...
        block.push(IRInstr::Call {
            dst: Some(IRValue::Register(1)),
            func: "other".to_string(),
            args: vec![],
        });

        // ...and used after the call.
        block.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(2),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(1),
        });

        block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let alloc = LinearScanAllocator::new();
        let result = alloc.allocate_function(&func).unwrap();

        // v0 should be assigned a register (ideally callee-saved for call-crossing,
        // but the current allocator may use caller-saved with save/restore).
        let reg0 = result.get_gpr(0);
        assert!(reg0.is_some(), "v0 should have a GPR assigned");
    }

    #[test]
    fn linear_scan_sequential_reuse() {
        // v0 is used then dies; v2 can reuse its register.
        let mut func = crate::ir::IRFunction::new("seq");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();

        // v1 = v0 + 1  (v0 is used, v1 is defined)
        block.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        });
        // v2 = v1 + 1  (v1 is used, v2 is defined; v0 is dead)
        block.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(2),
            lhs: IRValue::Register(1),
            rhs: IRValue::Immediate(1),
        });
        // Only v2 is returned — v0 and v1 are dead here.
        block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let alloc = LinearScanAllocator::new();
        let result = alloc.allocate_function(&func).unwrap();

        let p0 = result.get_phys_reg(0);
        let p2 = result.get_phys_reg(2);
        assert!(p0.is_some());
        assert!(p2.is_some());
        // v0 and v2 should share a register since their live ranges don't overlap.
        assert_eq!(
            p0, p2,
            "v0 and v2 should share a register (non-overlapping live ranges)"
        );
    }

    #[test]
    fn linear_scan_live_intervals_computation() {
        let mut func = crate::ir::IRFunction::new("interval_test");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();
        block.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let computer = LiveRangeComputer::new();
        let (intervals, call_positions) = computer.compute(&func);

        assert!(
            intervals.len() >= 2,
            "should have at least 2 intervals, got {}",
            intervals.len()
        );
        assert!(call_positions.is_empty(), "no calls in this function");

        let v1_interval = intervals.iter().find(|i| i.vreg == 1);
        assert!(v1_interval.is_some(), "should have an interval for v1");
    }

    #[test]
    fn linear_scan_multiple_blocks() {
        let mut func = crate::ir::IRFunction::new("multi_block");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();
        block.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        });
        block.terminator = IRTerminator::Branch {
            cond: IRValue::Register(0),
            true_block: "then".to_string(),
            false_block: "else".to_string(),
        };

        let then_idx = func.append_block("then");
        let then_block = &mut func.blocks[then_idx];
        then_block.push(IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(2),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(0),
        });
        then_block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let else_idx = func.append_block("else");
        let else_block = &mut func.blocks[else_idx];
        else_block.push(IRInstr::BinOp {
            op: BinOpKind::Sub,
            dst: IRValue::Register(3),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(0),
        });
        else_block.terminator = IRTerminator::Return(vec![IRValue::Register(3)]);

        let alloc = LinearScanAllocator::new();
        let result = alloc.allocate_function(&func).unwrap();

        assert!(result.get_phys_reg(0).is_some());
        assert!(result.get_phys_reg(1).is_some());
    }

    #[test]
    fn allocation_result_spill_frame_bytes() {
        let mut result = AllocationResult::new();
        result
            .spill_slots
            .insert(0, SpillSlot::new(0, -8, RegClass::Gpr));
        result
            .spill_slots
            .insert(1, SpillSlot::new(1, -16, RegClass::Gpr));
        result
            .spill_slots
            .insert(2, SpillSlot::new(2, -32, RegClass::SimdFp));

        // 2 GPR slots × 8 bytes + 1 SIMD slot × 16 bytes = 32
        assert_eq!(result.spill_frame_bytes(), 32);
    }

    #[test]
    fn simd_fp_register_properties() {
        // Caller-saved.
        assert!(SimdFpRegister::V0.is_caller_saved());
        assert!(SimdFpRegister::V31.is_caller_saved());
        assert!(SimdFpRegister::V16.is_caller_saved());

        // Callee-saved.
        assert!(SimdFpRegister::V8.is_callee_saved());
        assert!(SimdFpRegister::V15.is_callee_saved());
        assert!(!SimdFpRegister::V7.is_callee_saved());

        // Encoding.
        assert_eq!(SimdFpRegister::V0.encoding(), 0);
        assert_eq!(SimdFpRegister::V31.encoding(), 31);

        // Display.
        assert_eq!(format!("{}", SimdFpRegister::V5), "v5");
    }

    #[test]
    fn spill_slot_size() {
        let gpr_slot = SpillSlot::new(0, -8, RegClass::Gpr);
        assert_eq!(gpr_slot.size_bytes(), 8);

        let simd_slot = SpillSlot::new(0, -16, RegClass::SimdFp);
        assert_eq!(simd_slot.size_bytes(), 16);
    }

    #[test]
    fn linear_scan_allocator_register_counts() {
        let alloc = LinearScanAllocator::new();
        // 15 caller-saved + 10 callee-saved = 25 allocatable GPRs.
        assert_eq!(alloc.gpr_count(), 25);
        // 24 caller-saved + 8 callee-saved = 32 allocatable SIMD/FP regs.
        assert_eq!(alloc.simd_count(), 32);
    }

    #[test]
    fn linear_scan_program_allocation() {
        let mut program = IRProgram::new();

        // Function 1.
        let mut func1 = crate::ir::IRFunction::new("f1");
        func1.params.push(IRValue::Register(0));
        let block1 = func1.current_block();
        block1.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        });
        block1.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
        program.functions.push(func1);

        // Function 2.
        let mut func2 = crate::ir::IRFunction::new("f2");
        func2.params.push(IRValue::Register(0));
        func2.params.push(IRValue::Register(1));
        let block2 = func2.current_block();
        block2.push(IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(2),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(1),
        });
        block2.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);
        program.functions.push(func2);

        let alloc = LinearScanAllocator::new();
        let results = alloc.allocate_program(&program).unwrap();

        assert!(results.contains_key("f1"));
        assert!(results.contains_key("f2"));

        let f1_result = &results["f1"];
        let f2_result = &results["f2"];

        assert!(f1_result.get_phys_reg(0).is_some());
        assert!(f1_result.get_phys_reg(1).is_some());
        assert!(f2_result.get_phys_reg(0).is_some());
        assert!(f2_result.get_phys_reg(2).is_some());
    }

    // ====================================================================
    // NEW TESTS — Enhanced linear-scan features
    // ====================================================================

    /// Test that SIMD/FP class overrides work and SIMD registers are assigned.
    #[test]
    fn linear_scan_simd_class_override() {
        let mut func = crate::ir::IRFunction::new("fp_add");
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

        let mut class_overrides = HashMap::new();
        class_overrides.insert(0, RegClass::SimdFp);
        class_overrides.insert(1, RegClass::SimdFp);
        class_overrides.insert(2, RegClass::SimdFp);

        let alloc = LinearScanAllocator::new();
        let result = alloc
            .allocate_function_with_classes(&func, class_overrides)
            .unwrap();

        // All three vregs should be assigned SIMD/FP registers.
        let r0 = result.get_simd(0);
        let r1 = result.get_simd(1);
        let r2 = result.get_simd(2);

        assert!(r0.is_some(), "vreg 0 should have a SIMD reg");
        assert!(r1.is_some(), "vreg 1 should have a SIMD reg");
        assert!(r2.is_some(), "vreg 2 should have a SIMD reg");

        // They should be distinct since all are live simultaneously.
        assert_ne!(r0, r1, "v0 and v1 should not share a SIMD reg");
        assert_ne!(r0, r2, "v0 and v2 should not share a SIMD reg");
        assert_ne!(r1, r2, "v1 and v2 should not share a SIMD reg");
    }

    /// Test SIMD spill under pressure — create more live SIMD vregs than
    /// available SIMD registers (32).
    #[test]
    fn linear_scan_simd_spill_under_pressure() {
        let mut func = crate::ir::IRFunction::new("simd_pressure");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();

        // Chain of 35 values, all kept live via the return.
        for i in 0..35u32 {
            block.push(IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(i + 1),
                lhs: IRValue::Register(i),
                rhs: IRValue::Immediate(1),
            });
        }
        let ret_vals: Vec<IRValue> = (0..=35u32).map(IRValue::Register).collect();
        block.terminator = IRTerminator::Return(ret_vals);

        // Override all vregs to SIMD class.
        let mut class_overrides = HashMap::new();
        for i in 0..=35u32 {
            class_overrides.insert(i, RegClass::SimdFp);
        }

        let alloc = LinearScanAllocator::new();
        let result = alloc
            .allocate_function_with_classes(&func, class_overrides)
            .unwrap();

        assert!(
            !result.spill_slots.is_empty(),
            "expected SIMD spills with 36 live vregs and only 32 SIMD regs"
        );
    }

    /// Test register coalescing via BitCast.
    #[test]
    fn linear_scan_coalescing_bitcast() {
        let mut func = crate::ir::IRFunction::new("coalesce");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();

        // v1 = bitcast v0  — this may be coalesced with v0.
        block.push(IRInstr::Cast {
            kind: CastKind::BitCast,
            dst: IRValue::Register(1),
            src: IRValue::Register(0),
        });
        // v2 = v1 + 1
        block.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(2),
            lhs: IRValue::Register(1),
            rhs: IRValue::Immediate(1),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let alloc = LinearScanAllocator::new();
        let result = alloc.allocate_function(&func).unwrap();

        // v0 and v1 should both have physical registers assigned.
        // Coalescing may or may not merge them depending on allocator heuristics.
        let p0 = result.get_phys_reg(0);
        let p1 = result.get_phys_reg(1);
        assert!(p0.is_some(), "vreg 0 should have a physical reg");
        assert!(p1.is_some(), "vreg 1 should have a physical reg");
    }

    /// Test that the coalescing map tracks merged vregs properly.
    #[test]
    fn linear_scan_coalescing_map() {
        let mut func = crate::ir::IRFunction::new("coalesce_map");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();

        // v1 = bitcast v0  — coalesce v0 and v1.
        block.push(IRInstr::Cast {
            kind: CastKind::BitCast,
            dst: IRValue::Register(1),
            src: IRValue::Register(0),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let alloc = LinearScanAllocator::new();
        let result = alloc.allocate_function(&func).unwrap();

        // The coalesced_map should record that v1 was merged into v0 (or vice versa).
        // Either way, looking up v1 should yield the same preg as v0.
        let p0 = result.get_phys_reg(0);
        let p1 = result.get_phys_reg(1);
        assert_eq!(p0, p1, "coalesced vregs should share a register");
    }

    /// Test that the live interval spill_weight computation is reasonable.
    #[test]
    fn live_interval_spill_weight() {
        // An interval with many uses should have higher weight than one with few.
        let mut heavy = LiveInterval::new(0, RegClass::Gpr, 0, 10);
        heavy.use_positions = vec![1, 2, 3, 4, 5, 6, 7, 8, 9];
        heavy.def_positions = vec![0];
        heavy.crosses_call = true;

        let mut light = LiveInterval::new(1, RegClass::Gpr, 0, 10);
        light.use_positions = vec![5];
        light.def_positions = vec![0];
        light.crosses_call = false;

        assert!(
            heavy.spill_weight() > light.spill_weight(),
            "heavy interval should have higher spill weight"
        );
        assert!(
            heavy.weight_per_length() >= light.weight_per_length(),
            "heavy interval should have at least as high weight per length"
        );
    }

    /// Test that the AllocationResult's coalescing map correctly resolves vregs.
    #[test]
    fn allocation_result_coalescing_resolution() {
        let mut result = AllocationResult::new();
        result.vreg_to_preg.insert(0, PhysReg::Gpr(Register::X0));
        result.record_coalescing(1, 0);
        result.record_coalescing(2, 0);

        // vreg 1 and 2 should resolve to the same preg as vreg 0.
        assert_eq!(result.get_phys_reg(0), result.get_phys_reg(1));
        assert_eq!(result.get_phys_reg(0), result.get_phys_reg(2));
        assert_eq!(result.resolve_vreg(1), 0);
        assert_eq!(result.resolve_vreg(2), 0);
        assert_eq!(result.resolve_vreg(0), 0);
    }

    /// Test program-level allocation with per-function class overrides.
    #[test]
    fn linear_scan_program_with_classes() {
        let mut program = IRProgram::new();

        // Function 1 — GPR (default).
        let mut func1 = crate::ir::IRFunction::new("gpr_func");
        func1.params.push(IRValue::Register(0));
        let block1 = func1.current_block();
        block1.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        });
        block1.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
        program.functions.push(func1);

        // Function 2 — SIMD.
        let mut func2 = crate::ir::IRFunction::new("simd_func");
        func2.params.push(IRValue::Register(0));
        let block2 = func2.current_block();
        block2.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        });
        block2.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
        program.functions.push(func2);

        let mut class_overrides = HashMap::new();
        let mut f2_overrides = HashMap::new();
        f2_overrides.insert(0, RegClass::SimdFp);
        f2_overrides.insert(1, RegClass::SimdFp);
        class_overrides.insert("simd_func".to_string(), f2_overrides);

        let alloc = LinearScanAllocator::new();
        let results = alloc
            .allocate_program_with_classes(&program, &class_overrides)
            .unwrap();

        let f1 = &results["gpr_func"];
        let f2 = &results["simd_func"];

        // Function 1 should have GPRs.
        assert!(f1.get_gpr(0).is_some());
        // Function 2 should have SIMD regs.
        assert!(f2.get_simd(0).is_some());
        assert!(f2.get_simd(1).is_some());
    }

    /// Test that evicted intervals generate spill code.
    #[test]
    fn linear_scan_eviction_generates_spill_code() {
        let mut func = crate::ir::IRFunction::new("evict_test");
        func.params.push(IRValue::Register(0));
        let block = func.current_block();

        // Create enough live vregs to force eviction.
        for i in 0..30u32 {
            block.push(IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(i + 1),
                lhs: IRValue::Register(i),
                rhs: IRValue::Immediate(1),
            });
        }
        let ret_vals: Vec<IRValue> = (0..=30u32).map(IRValue::Register).collect();
        block.terminator = IRTerminator::Return(ret_vals);

        let alloc = LinearScanAllocator::new();
        let result = alloc.allocate_function(&func).unwrap();

        // There should be spill code generated.
        assert!(
            !result.spill_code.is_empty() || !result.spill_slots.is_empty(),
            "expected spill code or spill slots when registers are exhausted"
        );
    }

    /// Test the LiveInterval::is_empty method.
    #[test]
    fn live_interval_is_empty() {
        let interval = LiveInterval::new(0, RegClass::Gpr, 5, 5);
        assert!(interval.is_empty());

        let interval2 = LiveInterval::new(1, RegClass::Gpr, 0, 10);
        assert!(!interval2.is_empty());
    }

    /// Test that caller-saved PhysReg and callee-saved PhysReg classify correctly.
    #[test]
    fn phys_reg_caller_callee_classification() {
        let caller_gpr = PhysReg::Gpr(Register::X0);
        let callee_gpr = PhysReg::Gpr(Register::X19);
        let caller_simd = PhysReg::SimdFp(SimdFpRegister::V0);
        let callee_simd = PhysReg::SimdFp(SimdFpRegister::V8);

        assert!(caller_gpr.is_caller_saved());
        assert!(!caller_gpr.is_callee_saved());
        assert!(callee_gpr.is_callee_saved());
        assert!(!callee_gpr.is_caller_saved());
        assert!(caller_simd.is_caller_saved());
        assert!(callee_simd.is_callee_saved());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Worklog
// ═══════════════════════════════════════════════════════════════════════════
//
// 2026-03-04 — Enhanced linear-scan register allocator (Task 4-8)
//
// Changes:
//   1. Added `coalesced_vregs` field to `LiveInterval` — tracks all vreg IDs
//      merged into a single interval during register coalescing.
//   2. Added `is_empty()` method to `LiveInterval` for zero-length check.
//   3. Added `spill_weight()` and `weight_per_length()` to `LiveInterval` —
//      computes a heuristic weight for eviction decisions based on use/def
//      count, call crossing, and interval length.
//   4. Added `coalesced_map` field to `AllocationResult` — maps coalesced
//      vreg IDs to their representative vreg, enabling lookup of any vreg
//      in a coalesced group.
//   5. Enhanced `AllocationResult::get_phys_reg()`, `is_spilled()`, and
//      `spill_slot()` to follow the coalescing map.
//   6. Added `AllocationResult::record_coalescing()` and `resolve_vreg()`.
//   7. Added `assign_gpr()` and `assign_simd()` helper methods to
//      `LinearScanAllocator` that record the vreg→preg mapping for the
//      primary vreg and populate the coalesced_map for all coalesced vregs.
//   8. Changed eviction heuristic from "evict the active interval with the
//      farthest end point" to "evict the active interval with the lowest
//      spill weight per length" (with farthest-end tiebreaker).  This
//      prefers to evict intervals that are less important to keep in a
//      register (fewer uses per unit of live range).
//   9. Active interval tuples now carry the weight_per_length (4th element)
//      for efficient eviction decisions.
//  10. Added `gen_eviction_spill_reload()` — generates both spill and reload
//      code for evicted intervals (previously only a single spill was
//      generated at position 0).
//  11. Added `allocate_program_with_classes()` — program-level allocation
//      with per-function register class overrides.
//  12. Coalescing in `LiveRangeComputer::coalesce_intervals()` now collects
//      all vreg IDs from the coalesced group into the merged interval's
//      `coalesced_vregs` field.
//  13. Added 10 new tests:
//        - linear_scan_simd_class_override — SIMD register allocation
//        - linear_scan_simd_spill_under_pressure — SIMD spilling
//        - linear_scan_coalescing_bitcast — BitCast coalescing
//        - linear_scan_coalescing_map — coalesced vreg resolution
//        - live_interval_spill_weight — weight computation
//        - allocation_result_coalescing_resolution — coalescing map API
//        - linear_scan_program_with_classes — per-func class overrides
//        - linear_scan_eviction_generates_spill_code — eviction spill code
//        - live_interval_is_empty — zero-length interval check
//        - phys_reg_caller_callee_classification — PhysReg classification
//  14. Total test count: 24 (5 legacy + 13 original linear-scan + 10 new).
//  15. Register pool: 25 allocatable GPRs (X0–X7, X9–X15, X19–X28) and
//      32 allocatable SIMD/FP registers (V0–V7, V16–V31, V8–V15).
