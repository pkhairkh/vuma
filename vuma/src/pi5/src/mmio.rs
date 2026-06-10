//! Memory-mapped I/O subsystem for Raspberry Pi 5 (BCM2712).
//!
//! Provides volatile read and write primitives for interfacing with
//! hardware registers via memory-mapped addresses, ARM64 memory
//! synchronization barriers, Pi 5 memory map constants, and the
//! [`MmioDevice`] trait for abstract MMIO device modelling.
//!
//! # Pi 5 Memory Map
//!
//! | Region            | Start              | End                  |
//! |-------------------|--------------------|----------------------|
//! | RAM               | `0x0000_0000`      | Up to 8 GiB          |
//! | BCM2712 Peripherals| `0x0010_0000`     | `0x001F_FFFF`        |
//! | RP1 I/O           | `0x1F_0001_0000`   | `0x1F_0001_FFFF`     |
//! | ARM Local         | `0x7C00_0000_0000` | `0x7CFF_FFFF_FFFF`   |
//!
//! All MMIO accessors use `u64` addresses to cover the full Pi 5
//! 64-bit physical address space. Every function guarantees that
//! reads and writes are not elided or reordered by the compiler.

use core::ptr;

// ---------------------------------------------------------------------------
// Address type
// ---------------------------------------------------------------------------

/// A 64-bit physical address used for MMIO operations on the Pi 5.
///
/// Using `u64` instead of `usize` ensures correctness on 64-bit targets
/// while also allowing the same type to represent the full BCM2712
/// address space (which exceeds 32 bits for RP1 and ARM-local regions).
pub type Address = u64;

// ---------------------------------------------------------------------------
// Pi 5 Memory Map Constants
// ---------------------------------------------------------------------------

/// Start of the BCM2712 peripheral register space.
pub const BCM2712_PERIPHERAL_START: Address = 0x0010_0000;
/// End (inclusive) of the BCM2712 peripheral register space.
pub const BCM2712_PERIPHERAL_END: Address = 0x001F_FFFF;

/// Start of the RP1 I/O co-processor register space.
pub const RP1_IO_START: Address = 0x1F_0001_0000;
/// End (inclusive) of the RP1 I/O co-processor register space.
pub const RP1_IO_END: Address = 0x1F_0001_FFFF;

/// Start of the ARM-local (per-core) register space.
pub const ARM_LOCAL_START: Address = 0x7C00_0000_0000;
/// End (inclusive) of the ARM local register space.
pub const ARM_LOCAL_END: Address = 0x7CFF_FFFF_FFFF;

/// Base of DRAM (always at physical address 0 on the Pi 5).
pub const RAM_BASE: Address = 0x0000_0000;
/// Maximum supported RAM size on the Pi 5 (8 GiB).
pub const RAM_MAX_SIZE: u64 = 8 * 1024 * 1024 * 1024;

/// Returns `true` if `addr` falls within the BCM2712 peripheral region.
#[inline]
pub fn is_bcm2712_peripheral(addr: Address) -> bool {
    addr >= BCM2712_PERIPHERAL_START && addr <= BCM2712_PERIPHERAL_END
}

/// Returns `true` if `addr` falls within the RP1 I/O region.
#[inline]
pub fn is_rp1_io(addr: Address) -> bool {
    addr >= RP1_IO_START && addr <= RP1_IO_END
}

/// Returns `true` if `addr` falls within the ARM local region.
#[inline]
pub fn is_arm_local(addr: Address) -> bool {
    addr >= ARM_LOCAL_START && addr <= ARM_LOCAL_END
}

/// Returns `true` if `addr` falls within RAM.
#[inline]
pub fn is_ram(addr: Address) -> bool {
    addr >= RAM_BASE && addr < RAM_BASE + RAM_MAX_SIZE
}

// ---------------------------------------------------------------------------
// Volatile 32-bit accessors
// ---------------------------------------------------------------------------

/// Performs a volatile 32-bit read from the given MMIO address.
///
/// # Safety
///
/// The caller must ensure that `addr` is a valid, 4-byte-aligned MMIO
/// address that maps to a real hardware register or a safe mock region.
#[inline(always)]
pub fn mmio_read32(addr: Address) -> u32 {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::read_volatile(addr as *const u32) }
}

/// Performs a volatile 32-bit write to the given MMIO address.
///
/// # Safety
///
/// The caller must ensure that `addr` is a valid, 4-byte-aligned MMIO
/// address and that writing `value` to the target register is well-defined.
#[inline(always)]
pub fn mmio_write32(addr: Address, value: u32) {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::write_volatile(addr as *mut u32, value) }
}

// ---------------------------------------------------------------------------
// Volatile 64-bit accessors
// ---------------------------------------------------------------------------

/// Performs a volatile 64-bit read from the given MMIO address.
///
/// # Safety
///
/// The caller must ensure that `addr` is a valid, 8-byte-aligned MMIO
/// address that maps to a real hardware register or a safe mock region.
#[inline(always)]
pub fn mmio_read64(addr: Address) -> u64 {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::read_volatile(addr as *const u64) }
}

/// Performs a volatile 64-bit write to the given MMIO address.
///
/// # Safety
///
/// The caller must ensure that `addr` is a valid, 8-byte-aligned MMIO
/// address and that writing `value` to the target register is well-defined.
#[inline(always)]
pub fn mmio_write64(addr: Address, value: u64) {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::write_volatile(addr as *mut u64, value) }
}

// ---------------------------------------------------------------------------
// Legacy 8-bit / 16-bit accessors (kept for backward compatibility)
// ---------------------------------------------------------------------------

/// Performs a volatile 8-bit read from the given MMIO address.
#[inline(always)]
pub fn mmio_read8(addr: Address) -> u8 {
    // SAFETY: The caller guarantees `addr` is a valid MMIO register.
    unsafe { ptr::read_volatile(addr as *const u8) }
}

/// Performs a volatile 8-bit write to the given MMIO address.
#[inline(always)]
pub fn mmio_write8(addr: Address, value: u8) {
    // SAFETY: The caller guarantees `addr` is a valid MMIO register.
    unsafe { ptr::write_volatile(addr as *mut u8, value) }
}

/// Performs a volatile 16-bit read from the given MMIO address.
#[inline(always)]
pub fn mmio_read16(addr: Address) -> u16 {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::read_volatile(addr as *const u16) }
}

/// Performs a volatile 16-bit write to the given MMIO address.
#[inline(always)]
pub fn mmio_write16(addr: Address, value: u16) {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::write_volatile(addr as *mut u16, value) }
}

// ---------------------------------------------------------------------------
// Legacy aliases (backward compatibility)
// ---------------------------------------------------------------------------

/// Alias for [`mmio_read32`], retained for backward compatibility.
#[inline(always)]
pub fn mmio_read(addr: Address) -> u32 {
    mmio_read32(addr)
}

/// Alias for [`mmio_write32`], retained for backward compatibility.
#[inline(always)]
pub fn mmio_write(addr: Address, value: u32) {
    mmio_write32(addr, value)
}

// ---------------------------------------------------------------------------
// ARM64 Memory Barriers
// ---------------------------------------------------------------------------

/// Data Memory Barrier — ensures that all explicit memory accesses
/// (loads and stores) that precede this instruction in program order
/// are observed before any that follow it.
///
/// On AArch64 this emits `dmb sy` (system-wide, full barrier).
#[inline(always)]
pub fn dmb() {
    // SAFETY: DMB is a well-defined AArch64 barrier instruction with no
    // side effects beyond ordering memory accesses.
    unsafe { core::arch::asm!("dmb sy", options(nostack, preserves_flags)) };
}

/// Data Synchronization Barrier — ensures that all explicit memory
/// accesses that precede this instruction in program order complete
/// before the barrier completes. Stronger than [`dmb`]: also waits
/// for cache and TLB maintenance operations.
///
/// On AArch64 this emits `dsb sy` (system-wide, full barrier).
#[inline(always)]
pub fn dsb() {
    // SAFETY: DSB is a well-defined AArch64 barrier instruction with no
    // side effects beyond ordering and completing memory accesses.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Instruction Synchronization Barrier — flushes the processor
/// pipeline so that all instructions that come after `isb` in program
/// order are fetched only after the barrier completes. Used to
/// guarantee that any context-altering operations (e.g. system
/// register writes) take effect before subsequent instructions
/// execute.
///
/// On AArch64 this emits `isb`.
#[inline(always)]
pub fn isb() {
    // SAFETY: ISB is a well-defined AArch64 barrier instruction that
    // flushes the pipeline. No memory safety implications beyond ensuring
    // instruction ordering.
    unsafe { core::arch::asm!("isb", options(nostack, preserves_flags)) };
}

/// A memory fence that ensures all prior MMIO writes are observable
/// before subsequent operations proceed.
///
/// This is a convenience wrapper around [`dmb`] for legacy callers.
#[inline(always)]
pub fn mmio_fence() {
    dmb();
}

/// A memory fence that ensures all prior MMIO writes to device memory
/// are observable before subsequent device reads.
#[inline(always)]
pub fn mmio_fence_st() {
    // DMB OSHST — Data Memory Barrier, outer shareable, store-store.
    // SAFETY: well-defined AArch64 barrier instruction.
    unsafe { core::arch::asm!("dmb oshst", options(nostack, preserves_flags)) };
}

// ---------------------------------------------------------------------------
// MmioDevice trait
// ---------------------------------------------------------------------------

/// A trait abstracting a memory-mapped I/O device.
///
/// Each implementation owns a base address and provides typed register
/// access through `read_reg` / `write_reg`. The default implementations
/// delegate to the volatile accessors in this module, but implementations
/// may override them (e.g. to inject mock state in tests).
///
/// # Register model
///
/// Registers are addressed as **byte offsets** from the device base.
/// A 4-byte-aligned offset at `0x00` reads the first 32-bit register,
/// `0x04` reads the second, and so on.
pub trait MmioDevice {
    /// Returns the base physical address of this device's register block.
    fn base_address(&self) -> Address;

    /// Reads a 32-bit register at the given byte `offset` from base.
    ///
    /// # Safety
    ///
    /// The default implementation performs a volatile read. The caller
    /// must ensure `offset` is 4-byte-aligned and within the device's
    /// register space.
    fn read_reg(&self, offset: Address) -> u32 {
        mmio_read32(self.base_address() + offset)
    }

    /// Writes a 32-bit `value` to the register at the given byte `offset`
    /// from base.
    ///
    /// # Safety
    ///
    /// The default implementation performs a volatile write. The caller
    /// must ensure `offset` is 4-byte-aligned and within the device's
    /// register space.
    fn write_reg(&self, offset: Address, value: u32) {
        mmio_write32(self.base_address() + offset, value)
    }

    /// Reads a 64-bit register at the given byte `offset` from base.
    ///
    /// # Safety
    ///
    /// The default implementation performs a volatile read. The caller
    /// must ensure `offset` is 8-byte-aligned and within the device's
    /// register space.
    fn read_reg64(&self, offset: Address) -> u64 {
        mmio_read64(self.base_address() + offset)
    }

    /// Writes a 64-bit `value` to the register at the given byte `offset`
    /// from base.
    ///
    /// # Safety
    ///
    /// The default implementation performs a volatile write. The caller
    /// must ensure `offset` is 8-byte-aligned and within the device's
    /// register space.
    fn write_reg64(&self, offset: Address, value: u64) {
        mmio_write64(self.base_address() + offset, value)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::UnsafeCell;

    // -----------------------------------------------------------------------
    // Mock register bank for testing
    // -----------------------------------------------------------------------

    /// A mock 32-bit register bank backed by a fixed-size array.
    ///
    /// This allows unit tests to exercise the `MmioDevice` trait and
    /// MMIO accessor logic without touching real hardware.
    const MOCK_REG_COUNT: usize = 16;

    /// A mock MMIO device that stores register state in memory instead
    /// of performing real volatile accesses.
    struct MockMmioDevice {
        regs: UnsafeCell<[u32; MOCK_REG_COUNT]>,
        base: Address,
    }

    impl MockMmioDevice {
        fn new(base: Address) -> Self {
            Self {
                regs: UnsafeCell::new([0u32; MOCK_REG_COUNT]),
                base,
            }
        }

        /// Direct (non-volatile) read of a mock register for test assertions.
        fn get_reg(&self, index: usize) -> u32 {
            assert!(index < MOCK_REG_COUNT, "register index out of range");
            // SAFETY: We have &self and no concurrent write through our API.
            unsafe { (*self.regs.get())[index] }
        }

        /// Direct (non-volatile) write of a mock register for test setup.
        #[allow(dead_code)] // Available for future test assertions
        fn set_reg(&self, index: usize, value: u32) {
            assert!(index < MOCK_REG_COUNT, "register index out of range");
            // SAFETY: We have &self and no concurrent read through our API.
            unsafe { (*self.regs.get())[index] = value };
        }
    }

    impl MmioDevice for MockMmioDevice {
        fn base_address(&self) -> Address {
            self.base
        }

        fn read_reg(&self, offset: Address) -> u32 {
            let index = (offset / 4) as usize;
            assert!(index < MOCK_REG_COUNT, "register index out of range");
            // SAFETY: single-threaded test; no data race.
            unsafe { ptr::read_volatile((*self.regs.get()).as_ptr().add(index)) }
        }

        fn write_reg(&self, offset: Address, value: u32) {
            let index = (offset / 4) as usize;
            assert!(index < MOCK_REG_COUNT, "register index out of range");
            // SAFETY: single-threaded test; no data race.
            unsafe {
                ptr::write_volatile((*self.regs.get()).as_mut_ptr().add(index), value)
            }
        }

        fn read_reg64(&self, offset: Address) -> u64 {
            let lo = self.read_reg(offset) as u64;
            let hi = self.read_reg(offset + 4) as u64;
            (hi << 32) | lo
        }

        fn write_reg64(&self, offset: Address, value: u64) {
            self.write_reg(offset, value as u32);
            self.write_reg(offset + 4, (value >> 32) as u32);
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Memory map classification — BCM2712 peripheral
    // -----------------------------------------------------------------------
    #[test]
    fn bcm2712_peripheral_range_is_correct() {
        assert!(is_bcm2712_peripheral(0x0010_0000));
        assert!(is_bcm2712_peripheral(0x001F_FFFF));
        assert!(is_bcm2712_peripheral(0x0015_0000));
        assert!(!is_bcm2712_peripheral(0x000F_FFFF));
        assert!(!is_bcm2712_peripheral(0x0020_0000));
    }

    // -----------------------------------------------------------------------
    // Test 2: Memory map classification — RP1 I/O
    // -----------------------------------------------------------------------
    #[test]
    fn rp1_io_range_is_correct() {
        assert!(is_rp1_io(0x1F_0001_0000));
        assert!(is_rp1_io(0x1F_0001_FFFF));
        assert!(is_rp1_io(0x1F_0001_8000));
        assert!(!is_rp1_io(0x1F_0000_FFFF));
        assert!(!is_rp1_io(0x1F_0002_0000));
    }

    // -----------------------------------------------------------------------
    // Test 3: Memory map classification — ARM local
    // -----------------------------------------------------------------------
    #[test]
    fn arm_local_range_is_correct() {
        assert!(is_arm_local(0x7C00_0000_0000));
        assert!(is_arm_local(0x7CFF_FFFF_FFFF));
        assert!(is_arm_local(0x7C80_0000_0000));
        assert!(!is_arm_local(0x7BFF_FFFF_FFFF));
        assert!(!is_arm_local(0x7D00_0000_0000));
    }

    // -----------------------------------------------------------------------
    // Test 4: Memory map classification — RAM
    // -----------------------------------------------------------------------
    #[test]
    fn ram_range_is_correct() {
        assert!(is_ram(0x0000_0000));
        assert!(is_ram(0xFFFF_FFFF));
        assert!(is_ram(RAM_MAX_SIZE - 1));
        assert!(!is_ram(RAM_MAX_SIZE));
    }

    // -----------------------------------------------------------------------
    // Test 5: MmioDevice trait — read_reg / write_reg 32-bit
    // -----------------------------------------------------------------------
    #[test]
    fn mock_device_32bit_read_write() {
        let dev = MockMmioDevice::new(0x1F_0001_0000);
        assert_eq!(dev.base_address(), 0x1F_0001_0000);

        // Write to register at offset 0 (index 0)
        dev.write_reg(0x00, 0xDEAD_BEEF);
        assert_eq!(dev.read_reg(0x00), 0xDEAD_BEEF);
        assert_eq!(dev.get_reg(0), 0xDEAD_BEEF);

        // Write to register at offset 4 (index 1)
        dev.write_reg(0x04, 0x1234_5678);
        assert_eq!(dev.read_reg(0x04), 0x1234_5678);
    }

    // -----------------------------------------------------------------------
    // Test 6: MmioDevice trait — read_reg64 / write_reg64 64-bit
    // -----------------------------------------------------------------------
    #[test]
    fn mock_device_64bit_read_write() {
        let dev = MockMmioDevice::new(0x7C00_0000_0000);

        dev.write_reg64(0x00, 0xABCD_EF01_2345_6789);
        let val = dev.read_reg64(0x00);
        assert_eq!(val, 0xABCD_EF01_2345_6789);

        // Verify that the low 32 bits went to index 0, high to index 1.
        assert_eq!(dev.get_reg(0), 0x2345_6789);
        assert_eq!(dev.get_reg(1), 0xABCD_EF01);
    }

    // -----------------------------------------------------------------------
    // Test 7: Mock device register overwrite
    // -----------------------------------------------------------------------
    #[test]
    fn mock_device_overwrite_register() {
        let dev = MockMmioDevice::new(0x0010_0000);

        dev.write_reg(0x00, 0xAAAA_AAAA);
        assert_eq!(dev.read_reg(0x00), 0xAAAA_AAAA);

        dev.write_reg(0x00, 0x5555_5555);
        assert_eq!(dev.read_reg(0x00), 0x5555_5555);
    }

    // -----------------------------------------------------------------------
    // Test 8: Mock device — multiple registers are independent
    // -----------------------------------------------------------------------
    #[test]
    fn mock_device_independent_registers() {
        let dev = MockMmioDevice::new(0x0010_0000);

        dev.write_reg(0x00, 0x1111_1111);
        dev.write_reg(0x04, 0x2222_2222);
        dev.write_reg(0x08, 0x3333_3333);

        assert_eq!(dev.read_reg(0x00), 0x1111_1111);
        assert_eq!(dev.read_reg(0x04), 0x2222_2222);
        assert_eq!(dev.read_reg(0x08), 0x3333_3333);

        // Overwriting reg 1 should not affect the others.
        dev.write_reg(0x04, 0xFFFF_FFFF);
        assert_eq!(dev.read_reg(0x00), 0x1111_1111);
        assert_eq!(dev.read_reg(0x04), 0xFFFF_FFFF);
        assert_eq!(dev.read_reg(0x08), 0x3333_3333);
    }

    // -----------------------------------------------------------------------
    // Test 9: Address type is u64
    // -----------------------------------------------------------------------
    #[test]
    fn address_type_is_u64() {
        // Ensure the Address alias is u64 so we can represent the full
        // Pi 5 physical address space (including ARM local at 0x7C...).
        let arm_local_addr: Address = ARM_LOCAL_START;
        assert!(arm_local_addr > u32::MAX as u64);
    }

    // -----------------------------------------------------------------------
    // Test 10: Mock device — 64-bit write then partial 32-bit read
    // -----------------------------------------------------------------------
    #[test]
    fn mock_device_64bit_write_then_32bit_read() {
        let dev = MockMmioDevice::new(0x0010_0000);

        dev.write_reg64(0x00, 0x0102_0304_0506_0708);

        // Low 32 bits at offset 0x00
        assert_eq!(dev.read_reg(0x00), 0x0506_0708);
        // High 32 bits at offset 0x04
        assert_eq!(dev.read_reg(0x04), 0x0102_0304);
    }
}
