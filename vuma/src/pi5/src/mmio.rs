//! Memory-mapped I/O utilities for Raspberry Pi 5.
//!
//! Provides volatile read and write primitives for interfacing with
//! hardware registers via memory-mapped addresses. All functions guarantee
//! that reads and writes are not elided or reordered by the compiler.

use core::ptr;

/// A physical address used for MMIO operations.
pub type Address = usize;

/// Performs a volatile 32-bit read from the given MMIO address.
///
/// # Safety
///
/// The caller must ensure that `addr` is a valid, aligned MMIO address.
#[inline(always)]
pub fn mmio_read(addr: Address) -> u32 {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::read_volatile(addr as *const u32) }
}

/// Performs a volatile 32-bit write to the given MMIO address.
///
/// # Safety
///
/// The caller must ensure that `addr` is a valid, aligned MMIO address.
#[inline(always)]
pub fn mmio_write(addr: Address, value: u32) {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::write_volatile(addr as *mut u32, value) }
}

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

/// Performs a volatile 64-bit read from the given MMIO address.
#[inline(always)]
pub fn mmio_read64(addr: Address) -> u64 {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::read_volatile(addr as *const u64) }
}

/// Performs a volatile 64-bit write to the given MMIO address.
#[inline(always)]
pub fn mmio_write64(addr: Address, value: u64) {
    // SAFETY: The caller guarantees `addr` is a valid, aligned MMIO register.
    unsafe { ptr::write_volatile(addr as *mut u64, value) }
}

/// A memory fence that ensures all prior MMIO writes are observable
/// before subsequent operations proceed.
#[inline(always)]
pub fn mmio_fence() {
    // DMB SY — Data Memory Barrier, system-wide.
    // Ensures all explicit memory accesses before this instruction
    // are observed before any after it.
    core::arch::asm!("dmb sy", options(nostack, preserves_flags));
}

/// A memory fence that ensures all prior MMIO writes to device memory
/// are observable before subsequent device reads.
#[inline(always)]
pub fn mmio_fence_st() {
    // DMB OSHST — Data Memory Barrier, outer shareable, store-store.
    core::arch::asm!("dmb oshst", options(nostack, preserves_flags));
}
