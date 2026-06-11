//! GIC-400 Interrupt Controller driver for the BCM2712 (Raspberry Pi 5).
//!
//! The BCM2712 SoC uses an ARM GIC-400 (Generic Interrupt Controller, version 2)
//! to manage interrupts across the 4 Cortex-A76 cores. This module provides a
//! safe Rust interface to the GIC-400's Distributor and CPU Interface blocks.
//!
//! # BCM2712 GIC-400 Address Map
//!
//! | Block             | Base Address           | Size   |
//! |-------------------|------------------------|--------|
//! | GIC Distributor   | `0x7C00_4000_1000`     | 4 KiB  |
//! | GIC CPU Interface | `0x7C00_4001_0000`     | 8 KiB  |
//!
//! # BCM2712 Interrupt Assignments
//!
//! | Device  | IRQ ID | Type |
//! |---------|--------|------|
//! | Timer   | 30     | PPI  |
//! | UART    | 57     | SPI  |
//! | GPIO    | 145–152| SPI  |
//!
//! # Usage
//!
//! ```no_run
//! use vuma_pi5::gic::Gic400;
//!
//! let mut gic = Gic400::new();
//! gic.init();
//! gic.enable_irq(57);   // Enable UART interrupt
//! gic.set_priority(57, 0x80);
//! ```

use crate::mmio::{mmio_read32, mmio_write32, Address};

// ---------------------------------------------------------------------------
// BCM2712 GIC-400 base addresses
// ---------------------------------------------------------------------------

/// GIC-400 Distributor base address on BCM2712.
///
/// Located in the ARM Local register space at offset `0x4000_1000`.
pub const GICD_BASE: Address = 0x7C00_4000_1000;

/// GIC-400 CPU Interface base address on BCM2712.
///
/// Located in the ARM Local register space at offset `0x4001_0000`.
pub const GICC_BASE: Address = 0x7C00_4001_0000;

// ---------------------------------------------------------------------------
// BCM2712 Interrupt Assignments
// ---------------------------------------------------------------------------

/// ARM Generic Timer PPI interrupt ID on BCM2712.
pub const IRQ_TIMER: u32 = 30;

/// PL011 UART0 SPI interrupt ID on BCM2712.
pub const IRQ_UART: u32 = 57;

/// GPIO interrupt IDs on BCM2712 (8 GPIO bank interrupts).
pub const IRQ_GPIO_START: u32 = 145;
/// Last GPIO interrupt ID (inclusive).
pub const IRQ_GPIO_END: u32 = 152;

/// Maximum SPI (Shared Peripheral Interrupt) ID supported.
pub const MAX_IRQ: u32 = 255;

// ---------------------------------------------------------------------------
// GIC Distributor (GICD) register offsets
// ---------------------------------------------------------------------------

/// Distributor Control Register — enables/disables the distributor.
pub const GICD_CTLR: Address = 0x000;
/// Interrupt Controller Type Register — describes IRQ count and CPU count.
pub const GICD_TYPER: Address = 0x004;
/// Interrupt Set-Enable Register base — one bit per IRQ, 32 per register.
pub const GICD_ISENABLER: Address = 0x100;
/// Interrupt Clear-Enable Register base — one bit per IRQ, 32 per register.
pub const GICD_ICENABLER: Address = 0x180;
/// Interrupt Set-Pending Register base.
pub const GICD_ISPENDR: Address = 0x200;
/// Interrupt Clear-Pending Register base.
pub const GICD_ICPENDR: Address = 0x280;
/// Interrupt Priority Register base — one byte per IRQ.
pub const GICD_IPRIORITYR: Address = 0x400;
/// Interrupt Processor Targets Register base — one byte per IRQ.
pub const GICD_ITARGETSR: Address = 0x800;
/// Interrupt Configuration Register base — 2 bits per IRQ.
pub const GICD_ICFGR: Address = 0xC00;

// ---------------------------------------------------------------------------
// GIC CPU Interface (GICC) register offsets
// ---------------------------------------------------------------------------

/// CPU Interface Control Register — enables/disables signaling to the core.
pub const GICC_CTLR: Address = 0x000;
/// Interrupt Priority Mask Register — minimum priority to signal.
pub const GICC_PMR: Address = 0x004;
/// Binary Point Register — priority grouping.
pub const GICC_BPR: Address = 0x008;
/// Interrupt Acknowledge Register — returns the highest-priority pending IRQ.
pub const GICC_IAR: Address = 0x00C;
/// End of Interrupt Register — signals completion of IRQ handling.
pub const GICC_EOIR: Address = 0x010;
/// Running Priority Register — current active priority.
pub const GICC_RPR: Address = 0x014;
/// Highest Priority Pending Interrupt Register.
pub const GICC_HPPIR: Address = 0x018;

// ---------------------------------------------------------------------------
// GICD_CTLR bits
// ---------------------------------------------------------------------------

/// Enable bit for the GIC Distributor.
pub const GICD_CTLR_ENABLE: u32 = 1;

// ---------------------------------------------------------------------------
// GICC_CTLR bits
// ---------------------------------------------------------------------------

/// Enable bit for the CPU Interface.
pub const GICC_CTLR_ENABLE: u32 = 1;

// ---------------------------------------------------------------------------
// GICC_IAR special value
// ---------------------------------------------------------------------------

/// Returned by IAR when no pending interrupt exists.
pub const IAR_SPURIOUS: u32 = 1023;

// ---------------------------------------------------------------------------
// Priority / target constants
// ---------------------------------------------------------------------------

/// Default priority value (mid-range, 0 = highest, 0xFF = lowest).
pub const DEFAULT_PRIORITY: u32 = 0x80;

/// Default CPU target mask — route to core 0.
pub const TARGET_CORE0: u32 = 0x01;

// ---------------------------------------------------------------------------
// GIC-400 Driver
// ---------------------------------------------------------------------------

/// A driver for the ARM GIC-400 interrupt controller on the BCM2712.
///
/// The `Gic400` struct holds the base addresses of the Distributor and
/// CPU Interface register blocks. All register access uses volatile MMIO
/// reads and writes via the [`mmio_read32`] / [`mmio_write32`] primitives.
#[derive(Debug, Clone, Copy)]
pub struct Gic400 {
    /// Base address of the GIC Distributor register block.
    gicd_base: Address,
    /// Base address of the GIC CPU Interface register block.
    gicc_base: Address,
}

impl Gic400 {
    /// Creates a new `Gic400` driver using the default BCM2712 base addresses.
    #[inline]
    pub const fn new() -> Self {
        Self {
            gicd_base: GICD_BASE,
            gicc_base: GICC_BASE,
        }
    }

    /// Creates a `Gic400` driver with custom base addresses.
    ///
    /// Useful for testing with mock addresses.
    #[inline]
    pub const fn with_bases(gicd_base: Address, gicc_base: Address) -> Self {
        Self {
            gicd_base,
            gicc_base,
        }
    }

    /// Returns the Distributor base address.
    #[inline]
    pub const fn gicd_base(&self) -> Address {
        self.gicd_base
    }

    /// Returns the CPU Interface base address.
    #[inline]
    pub const fn gicc_base(&self) -> Address {
        self.gicc_base
    }

    // -----------------------------------------------------------------------
    // Initialisation
    // -----------------------------------------------------------------------

    /// Initialises the GIC-400 for use on core 0.
    ///
    /// This performs the following steps:
    ///
    /// 1. Disables the Distributor and CPU Interface.
    /// 2. Sets all interrupt priorities to the default (mid-range).
    /// 3. Routes all SPIs (IRQs 32+) to core 0.
    /// 4. Disables all interrupts.
    /// 5. Clears all pending states.
    /// 6. Enables the Distributor and CPU Interface.
    /// 7. Sets the priority mask to allow all priorities.
    /// 8. Sets the binary point to the minimum group priority.
    pub fn init(&self) {
        // 1. Disable distributor and CPU interface.
        mmio_write32(self.gicd_base + GICD_CTLR, 0);
        mmio_write32(self.gicc_base + GICC_CTLR, 0);

        // 2. Set default priority for all IRQs.
        for irq in 0..=MAX_IRQ {
            self.set_priority_raw(irq, DEFAULT_PRIORITY);
        }

        // 3. Route all SPIs (IRQs 32+) to core 0.
        for irq in 32..=MAX_IRQ {
            self.set_target_raw(irq, TARGET_CORE0);
        }

        // 4. Disable all interrupts.
        for n in 0..8 {
            mmio_write32(self.gicd_base + GICD_ICENABLER + n * 4, 0xFFFF_FFFF);
        }

        // 5. Clear all pending states.
        for n in 0..8 {
            mmio_write32(self.gicd_base + GICD_ICPENDR + n * 4, 0xFFFF_FFFF);
        }

        // 6. Enable the distributor.
        mmio_write32(self.gicd_base + GICD_CTLR, GICD_CTLR_ENABLE);

        // 7. Enable the CPU interface, set priority mask, and binary point.
        mmio_write32(self.gicc_base + GICC_PMR, 0xFF);
        mmio_write32(self.gicc_base + GICC_BPR, 0);
        mmio_write32(self.gicc_base + GICC_CTLR, GICC_CTLR_ENABLE);
    }

    // -----------------------------------------------------------------------
    // Interrupt enable / disable
    // -----------------------------------------------------------------------

    /// Enables the specified IRQ.
    ///
    /// The IRQ is identified by its global ID (0–255 for GIC-400).
    /// Each `ISENABLER` register covers 32 IRQs; the correct register
    /// is selected by `irq / 32` and the bit by `1 << (irq % 32)`.
    pub fn enable_irq(&self, irq: u32) {
        if irq > MAX_IRQ {
            return;
        }
        let reg_offset = GICD_ISENABLER + ((irq / 32) as Address) * 4;
        let bit = 1u32 << (irq % 32);
        mmio_write32(self.gicd_base + reg_offset, bit);
    }

    /// Disables the specified IRQ.
    ///
    /// Uses the `ICENABLER` register (write-1-to-clear model).
    pub fn disable_irq(&self, irq: u32) {
        if irq > MAX_IRQ {
            return;
        }
        let reg_offset = GICD_ICENABLER + ((irq / 32) as Address) * 4;
        let bit = 1u32 << (irq % 32);
        mmio_write32(self.gicd_base + reg_offset, bit);
    }

    // -----------------------------------------------------------------------
    // Interrupt acknowledge / end-of-interrupt
    // -----------------------------------------------------------------------

    /// Acknowledges the highest-priority pending interrupt.
    ///
    /// Reads the Interrupt Acknowledge Register (IAR), which returns the
    /// IRQ ID of the highest-priority pending interrupt. Returns
    /// [`IAR_SPURIOUS`] (1023) if no interrupt is pending.
    pub fn acknowledge_irq(&self) -> u32 {
        mmio_read32(self.gicc_base + GICC_IAR)
    }

    /// Signals end-of-interrupt for the given IRQ ID.
    ///
    /// Must be called after the interrupt handler has finished processing
    /// the interrupt identified by the value returned from [`acknowledge_irq`].
    pub fn end_of_irq(&self, irq: u32) {
        mmio_write32(self.gicc_base + GICC_EOIR, irq);
    }

    // -----------------------------------------------------------------------
    // Priority
    // -----------------------------------------------------------------------

    /// Sets the priority for the specified IRQ.
    ///
    /// Lower values indicate higher priority (0 = highest, 255 = lowest).
    /// The BCM2712 GIC-400 supports 8 priority levels with 32-step
    /// granularity (0x00, 0x20, 0x40, …, 0xE0).
    pub fn set_priority(&self, irq: u32, priority: u32) {
        if irq > MAX_IRQ {
            return;
        }
        self.set_priority_raw(irq, priority & 0xFF);
    }

    /// Returns the priority for the specified IRQ.
    pub fn get_priority(&self, irq: u32) -> u32 {
        if irq > MAX_IRQ {
            return DEFAULT_PRIORITY;
        }
        self.get_priority_raw(irq)
    }

    // -----------------------------------------------------------------------
    // Pending state
    // -----------------------------------------------------------------------

    /// Returns the ID of the highest-priority pending interrupt, or `None`
    /// if no interrupt is pending (spurious).
    pub fn get_pending_irq(&self) -> Option<u32> {
        let iar = self.acknowledge_irq();
        if iar >= IAR_SPURIOUS {
            None
        } else {
            Some(iar)
        }
    }

    // -----------------------------------------------------------------------
    // Type information
    // -----------------------------------------------------------------------

    /// Reads the GICD_TYPER register and returns the number of interrupt
    /// lines supported.
    ///
    /// The ITLinesNumber field (bits [4:0]) encodes the count as
    /// `(ITLinesNumber + 1) * 32`.
    pub fn typer_irq_count(&self) -> u32 {
        let typer = mmio_read32(self.gicd_base + GICD_TYPER);
        let it_lines_number = typer & 0x1F;
        (it_lines_number + 1) * 32
    }

    // -----------------------------------------------------------------------
    // Internal helpers (no bounds checks — used in init loop)
    // -----------------------------------------------------------------------

    /// Writes the priority byte for the given IRQ.
    #[inline]
    fn set_priority_raw(&self, irq: u32, priority: u32) {
        let offset = GICD_IPRIORITYR + irq as Address;
        mmio_write32(self.gicd_base + offset, priority & 0xFF);
    }

    /// Reads the priority byte for the given IRQ.
    #[inline]
    fn get_priority_raw(&self, irq: u32) -> u32 {
        let offset = GICD_IPRIORITYR + irq as Address;
        mmio_read32(self.gicd_base + offset) & 0xFF
    }

    /// Writes the CPU target byte for the given IRQ.
    #[inline]
    fn set_target_raw(&self, irq: u32, target: u32) {
        let offset = GICD_ITARGETSR + irq as Address;
        mmio_write32(self.gicd_base + offset, target & 0xFF);
    }
}

impl Default for Gic400 {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helper functions for register offset calculation (pure, testable)
// ---------------------------------------------------------------------------

/// Returns the `ISENABLER` register offset and bit mask for the given IRQ.
///
/// Pure calculation — no MMIO access.
#[inline]
pub const fn isenabler_offset_and_bit(irq: u32) -> (Address, u32) {
    let reg_index = irq / 32;
    let bit = 1u32 << (irq % 32);
    (GICD_ISENABLER + (reg_index as Address) * 4, bit)
}

/// Returns the `ICENABLER` register offset and bit mask for the given IRQ.
///
/// Pure calculation — no MMIO access.
#[inline]
pub const fn icenabler_offset_and_bit(irq: u32) -> (Address, u32) {
    let reg_index = irq / 32;
    let bit = 1u32 << (irq % 32);
    (GICD_ICENABLER + (reg_index as Address) * 4, bit)
}

/// Returns the `IPRIORITYR` byte offset for the given IRQ.
///
/// Pure calculation — no MMIO access.
#[inline]
pub const fn ipriorityr_offset(irq: u32) -> Address {
    GICD_IPRIORITYR + (irq as Address)
}

/// Returns the `ITARGETSR` byte offset for the given IRQ.
///
/// Pure calculation — no MMIO access.
#[inline]
pub const fn itargetsr_offset(irq: u32) -> Address {
    GICD_ITARGETSR + (irq as Address)
}

/// Returns the `ICFGR` half-word offset for the given IRQ.
///
/// Pure calculation — no MMIO access.
#[inline]
pub const fn icfgr_offset(irq: u32) -> Address {
    GICD_ICFGR + ((irq / 16) as Address) * 4
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test 1: Gic400 default base addresses match BCM2712 constants
    // -----------------------------------------------------------------------
    #[test]
    fn gic400_default_bases_match_bcm2712() {
        let gic = Gic400::new();
        assert_eq!(gic.gicd_base(), GICD_BASE);
        assert_eq!(gic.gicc_base(), GICC_BASE);
    }

    // -----------------------------------------------------------------------
    // Test 2: Gic400 custom base addresses
    // -----------------------------------------------------------------------
    #[test]
    fn gic400_custom_bases() {
        let gic = Gic400::with_bases(0xDEAD_0000, 0xBEEF_0000);
        assert_eq!(gic.gicd_base(), 0xDEAD_0000);
        assert_eq!(gic.gicc_base(), 0xBEEF_0000);
    }

    // -----------------------------------------------------------------------
    // Test 3: BCM2712 interrupt assignments are correct
    // -----------------------------------------------------------------------
    #[test]
    fn bcm2712_interrupt_assignments() {
        assert_eq!(IRQ_TIMER, 30, "Timer IRQ should be 30");
        assert_eq!(IRQ_UART, 57, "UART IRQ should be 57");
        assert_eq!(IRQ_GPIO_START, 145, "GPIO IRQ start should be 145");
        assert_eq!(IRQ_GPIO_END, 152, "GPIO IRQ end should be 152");
        assert_eq!(
            IRQ_GPIO_END - IRQ_GPIO_START + 1,
            8,
            "Should be 8 GPIO interrupts"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: ISENABLER offset and bit calculation
    // -----------------------------------------------------------------------
    #[test]
    fn isenabler_offset_and_bit_calc() {
        // IRQ 57 (UART): reg_index = 57/32 = 1, bit = 1 << (57%32) = 1 << 25
        let (offset, bit) = isenabler_offset_and_bit(IRQ_UART);
        assert_eq!(offset, GICD_ISENABLER + 4);
        assert_eq!(bit, 1u32 << 25);

        // IRQ 0: reg_index = 0, bit = 1 << 0 = 1
        let (offset0, bit0) = isenabler_offset_and_bit(0);
        assert_eq!(offset0, GICD_ISENABLER);
        assert_eq!(bit0, 1);

        // IRQ 31: reg_index = 0, bit = 1 << 31
        let (offset31, bit31) = isenabler_offset_and_bit(31);
        assert_eq!(offset31, GICD_ISENABLER);
        assert_eq!(bit31, 1u32 << 31);

        // IRQ 32: reg_index = 1, bit = 1 << 0
        let (offset32, bit32) = isenabler_offset_and_bit(32);
        assert_eq!(offset32, GICD_ISENABLER + 4);
        assert_eq!(bit32, 1);
    }

    // -----------------------------------------------------------------------
    // Test 5: ICENABLER offset and bit calculation
    // -----------------------------------------------------------------------
    #[test]
    fn icenabler_offset_and_bit_calc() {
        // IRQ 30 (Timer): reg_index = 0, bit = 1 << 30
        let (offset, bit) = icenabler_offset_and_bit(IRQ_TIMER);
        assert_eq!(offset, GICD_ICENABLER);
        assert_eq!(bit, 1u32 << 30);
    }

    // -----------------------------------------------------------------------
    // Test 6: Priority register offset calculation
    // -----------------------------------------------------------------------
    #[test]
    fn ipriorityr_offset_calc() {
        assert_eq!(ipriorityr_offset(0), GICD_IPRIORITYR);
        assert_eq!(ipriorityr_offset(57), GICD_IPRIORITYR + 57);
        assert_eq!(ipriorityr_offset(255), GICD_IPRIORITYR + 255);
    }

    // -----------------------------------------------------------------------
    // Test 7: Target register offset calculation
    // -----------------------------------------------------------------------
    #[test]
    fn itargetsr_offset_calc() {
        assert_eq!(itargetsr_offset(0), GICD_ITARGETSR);
        assert_eq!(itargetsr_offset(32), GICD_ITARGETSR + 32);
    }

    // -----------------------------------------------------------------------
    // Test 8: ICFGR offset calculation and MAX_IRQ boundary
    // -----------------------------------------------------------------------
    #[test]
    fn icfgr_offset_and_max_irq() {
        // 16 IRQs per ICFGR register
        assert_eq!(icfgr_offset(0), GICD_ICFGR);
        assert_eq!(icfgr_offset(15), GICD_ICFGR);
        assert_eq!(icfgr_offset(16), GICD_ICFGR + 4);
        assert_eq!(icfgr_offset(32), GICD_ICFGR + 8);

        // MAX_IRQ is 255 (GIC-400 supports up to this)
        assert_eq!(MAX_IRQ, 255);

        // Spurious interrupt ID
        assert_eq!(IAR_SPURIOUS, 1023);
    }

    // -----------------------------------------------------------------------
    // Test 9: GicD / GicC register offsets are correctly spaced
    // -----------------------------------------------------------------------
    #[test]
    fn register_offsets_are_correct() {
        // Distributor
        assert_eq!(GICD_CTLR, 0x000);
        assert_eq!(GICD_TYPER, 0x004);
        assert_eq!(GICD_ISENABLER, 0x100);
        assert_eq!(GICD_ICENABLER, 0x180);
        assert_eq!(GICD_ISPENDR, 0x200);
        assert_eq!(GICD_ICPENDR, 0x280);
        assert_eq!(GICD_IPRIORITYR, 0x400);
        assert_eq!(GICD_ITARGETSR, 0x800);
        assert_eq!(GICD_ICFGR, 0xC00);

        // CPU Interface
        assert_eq!(GICC_CTLR, 0x000);
        assert_eq!(GICC_PMR, 0x004);
        assert_eq!(GICC_BPR, 0x008);
        assert_eq!(GICC_IAR, 0x00C);
        assert_eq!(GICC_EOIR, 0x010);
        assert_eq!(GICC_RPR, 0x014);
        assert_eq!(GICC_HPPIR, 0x018);
    }
}
