//! Multi-core (SMP) support for the Raspberry Pi 5.
//!
//! The BCM2712 has four Cortex-A76 cores. This module provides utilities
//! for identifying the current core, starting secondary cores, and
//! inter-core communication via mailbox registers.

use crate::mmio::{mmio_read, mmio_write, Address};
use crate::platform::NUM_CORES;

// ---------------------------------------------------------------------------
// Core identification
// ---------------------------------------------------------------------------

/// A zero-based core identifier (0..3) on the BCM2712.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct CoreId(u8);

impl CoreId {
    /// Core 0 (the boot core).
    pub const CORE0: Self = Self(0);
    /// Core 1.
    pub const CORE1: Self = Self(1);
    /// Core 2.
    pub const CORE2: Self = Self(2);
    /// Core 3.
    pub const CORE3: Self = Self(3);

    /// All valid core IDs.
    pub const ALL: [CoreId; NUM_CORES] = [Self::CORE0, Self::CORE1, Self::CORE2, Self::CORE3];

    /// Creates a `CoreId` from a raw value, returning `None` if out of range.
    pub const fn from_raw(id: u8) -> Option<Self> {
        if id < NUM_CORES as u8 {
            Some(Self(id))
        } else {
            None
        }
    }

    /// Creates a `CoreId` without checking bounds.
    ///
    /// # Safety
    ///
    /// `id` must be < `NUM_CORES`.
    pub const unsafe fn from_raw_unchecked(id: u8) -> Self {
        Self(id)
    }

    /// Returns the raw integer value.
    #[inline]
    pub const fn as_u8(&self) -> u8 {
        self.0
    }

    /// Returns the raw integer value as `usize`.
    #[inline]
    pub const fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

// ---------------------------------------------------------------------------
// Mailbox / spin-table registers for secondary core boot
// ---------------------------------------------------------------------------

/// Base address of the ARM local peripherals.
/// On the BCM2712 this is at 0xFF800_0000 (or 0x7F800_0000 in
/// low-peripheral mode). These offsets are relative to that base.
///
/// For the Pi 5, the VideoCore uses a spin-table mechanism to start
/// secondary cores. Each core has a 64-bit entry-point register and a
/// 64-bit context-id register.
///
/// Offsets relative to the local peripheral base:
pub const LOCAL_PERIPH_BASE: usize = 0xFF800_0000;

/// Offset from LOCAL_PERIPH_BASE to core 0 mailbox register set.
/// Each core gets a set of 4 × 32-bit mailbox registers.
const MAILBOX_STRIDE: usize = 0x10; // 16 bytes per core set

/// Mailbox 0 register for core N (read/write).
/// Address: LOCAL_PERIPH_BASE + 0x00 + N * MAILBOX_STRIDE
const MAILBOX0_SET0: usize = 0x00;

/// Mailbox 1 register for core N.
const MAILBOX1_SET0: usize = 0x04;

/// Mailbox 2 register for core N.
const MAILBOX2_SET0: usize = 0x08;

/// Mailbox 3 register for core N.
const MAILBOX3_SET0: usize = 0x0C;

// For the Pi 5 / BCM2712, secondary core boot uses a PSCI-style or
// spin-table mechanism. The typical approach is:
//
//   1. Write the entry-point address to a known mailbox location.
//   2. Send an event (SEV) to wake the waiting core.
//
// The mailbox registers are in the ARM local peripheral space.

/// Offset from LOCAL_PERIPH_BASE to the spin-table entry-point for core N.
///
/// On the BCM2712 these are at offsets 0xD0 + (N * 8).
const SPIN_TABLE_BASE: usize = 0xD0;

// ---------------------------------------------------------------------------
// SMP functions
// ---------------------------------------------------------------------------

/// Returns the ID of the currently executing core by reading `MPIDR_EL1`.
///
/// On the BCM2712 the Aff0 field contains the core number (0–3).
#[inline(always)]
pub fn current_core() -> CoreId {
    let mpidr: u64;
    // SAFETY: MPIDR_EL1 is a readable system register available on all
    // AArch64 implementations.
    core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nostack, preserves_flags));
    // Mask off everything except Aff0 (bits [7:0]), which holds the core ID.
    let core_id = (mpidr & 0xFF) as u8;
    // In well-formed firmware Aff0 is 0..3.
    CoreId(core_id)
}

/// Starts the specified secondary core, directing it to execute from
/// `entry_point`.
///
/// # How it works
///
/// 1. Writes the physical address of `entry_point` to the spin-table
///    location for the target core.
/// 2. Issues a `SEV` (Send Event) instruction so that any core
///    spinning in `WFE` will wake and branch to the entry point.
///
/// # Safety
///
/// The caller must ensure:
/// - The `entry_point` points to valid executable code.
/// - The target core has not already been started.
/// - Proper synchronisation is in place for shared resources.
pub fn start_core(id: CoreId, entry_point: usize) {
    let addr = LOCAL_PERIPH_BASE + SPIN_TABLE_BASE + id.as_usize() * 8;

    // Write the entry point to the spin table for this core.
    // Use 64-bit write since the spin-table entries are 64 bits wide.
    unsafe {
        core::ptr::write_volatile(addr as *mut u64, entry_point as u64);
    }

    // Ensure the write is visible before waking the core.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    // Send an event to wake cores waiting in WFE.
    core::arch::asm!("sev", options(nostack, preserves_flags));
}

/// Puts the calling core into a low-power wait (`WFE`) until an event
/// is received (e.g. via `SEV` from another core or an interrupt).
///
/// This is typically used by secondary cores at boot to wait for the
/// primary core to set their entry point.
#[inline(always)]
pub fn spin_wait(_id: CoreId) {
    // Loop in WFE — the core will wake on SEV or interrupt.
    loop {
        core::arch::asm!("wfe", options(nostack, preserves_flags));
        // Check if the spin table entry is non-zero; if so, the core
        // has been given work to do. The actual branching out of this
        // loop would be done by platform-specific boot code.
    }
}

/// Sends an inter-core interrupt / mailbox message to the specified core.
///
/// Writes `value` to mailbox register 0 of the target core and issues
/// an event (`SEV`) so the core is woken from `WFE` if it was sleeping.
pub fn inter_core_interrupt(id: CoreId, value: u32) {
    let addr = LOCAL_PERIPH_BASE + MAILBOX0_SET0 + id.as_usize() * MAILBOX_STRIDE;
    mmio_write(addr, value);

    // Ensure the write is visible before waking the core.
    crate::mmio::mmio_fence();

    // Wake the target core.
    core::arch::asm!("sev", options(nostack, preserves_flags));
}

/// Reads the pending mailbox value for the specified core (mailbox 0).
#[inline]
pub fn read_mailbox(id: CoreId) -> u32 {
    let addr = LOCAL_PERIPH_BASE + MAILBOX0_SET0 + id.as_usize() * MAILBOX_STRIDE;
    mmio_read(addr)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_id_from_raw_valid() {
        assert_eq!(CoreId::from_raw(0), Some(CoreId::CORE0));
        assert_eq!(CoreId::from_raw(3), Some(CoreId::CORE3));
    }

    #[test]
    fn core_id_from_raw_invalid() {
        assert_eq!(CoreId::from_raw(4), None);
        assert_eq!(CoreId::from_raw(255), None);
    }

    #[test]
    fn core_id_ordering() {
        assert!(CoreId::CORE0 < CoreId::CORE1);
        assert!(CoreId::CORE2 < CoreId::CORE3);
    }

    #[test]
    fn all_cores_count() {
        assert_eq!(CoreId::ALL.len(), NUM_CORES);
    }
}
