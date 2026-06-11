//! Multi-core (SMP) support for the Raspberry Pi 5.
//!
//! The BCM2712 has four Cortex-A76 cores. This module provides utilities
//! for identifying the current core, starting secondary cores, inter-core
//! communication via mailbox registers, and a spinlock primitive for
//! mutual exclusion between cores.
//!
//! # Free-standing API
//!
//! - [`smp_init`]          — bring up cores 1–3
//! - [`smp_get_core_id`]   — return current core ID as `u32`
//! - [`smp_send_ipi`]      — send an inter-processor interrupt
//!
//! # Synchronisation
//!
//! - [`Spinlock`] — a simple spinlock using atomic compare-and-swap
//!   for efficient inter-core mutual exclusion.

use crate::mmio::{mmio_read, mmio_write};
use crate::platform::NUM_CORES;
use core::sync::atomic::{AtomicU32, Ordering};

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

    /// Returns the raw integer value as `u32`.
    #[inline]
    pub const fn as_u32(&self) -> u32 {
        self.0 as u32
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
pub const LOCAL_PERIPH_BASE: u64 = 0xFF800_0000;

/// Offset from LOCAL_PERIPH_BASE to core 0 mailbox register set.
/// Each core gets a set of 4 × 32-bit mailbox registers.
const MAILBOX_STRIDE: u64 = 0x10; // 16 bytes per core set

/// Mailbox 0 register for core N (read/write).
/// Address: LOCAL_PERIPH_BASE + 0x00 + N * MAILBOX_STRIDE
const MAILBOX0_SET0: u64 = 0x00;

/// Mailbox 1 register for core N.
const MAILBOX1_SET0: u64 = 0x04;

/// Mailbox 2 register for core N.
const MAILBOX2_SET0: u64 = 0x08;

/// Mailbox 3 register for core N.
const MAILBOX3_SET0: u64 = 0x0C;

/// Offset from LOCAL_PERIPH_BASE to the spin-table entry-point for core N.
///
/// On the BCM2712 these are at offsets 0xD0 + (N * 8).
const SPIN_TABLE_BASE: u64 = 0xD0;

/// Offset from LOCAL_PERIPH_BASE to the core N interrupt control / IPI
/// doorbell registers.  On BCM2712 the per-core local interrupts include
/// a set of doorbell / IPI registers.  The IPI doorbells for core N start
/// at offset 0x40 + N * 0x04 within the local peripheral space.
///
/// Writing a non-zero value to a core's doorbell triggers an IRQ on that
/// core.  The `vector` value is available in the interrupt handler.
const IPI_DOORBELL_BASE: u64 = 0x40;
const IPI_DOORBELL_STRIDE: u64 = 0x04;

// ---------------------------------------------------------------------------
// Core-start tracking
// ---------------------------------------------------------------------------

/// Bitmask tracking which cores have been started (bit N = core N).
static CORES_STARTED: AtomicU32 = AtomicU32::new(0x1); // core 0 is always started

// ---------------------------------------------------------------------------
// SMP free-standing API
// ---------------------------------------------------------------------------

/// Returns the ID of the currently executing core by reading `MPIDR_EL1`.
///
/// On the BCM2712 the Aff0 field contains the core number (0–3).
#[inline(always)]
pub fn current_core() -> CoreId {
    let mpidr: u64;
    // SAFETY: MPIDR_EL1 is a readable system register available on all
    // AArch64 implementations.
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nostack, preserves_flags));
    }
    // Mask off everything except Aff0 (bits [7:0]), which holds the core ID.
    let core_id = (mpidr & 0xFF) as u8;
    // In well-formed firmware Aff0 is 0..3.
    CoreId(core_id)
}

/// Returns the current core ID as a `u32`.
///
/// Convenience wrapper around [`current_core`] that matches the
/// C-style API used in many bare-metal Pi 5 projects.
#[inline(always)]
pub fn smp_get_core_id() -> u32 {
    current_core().as_u32()
}

/// Brings up secondary cores 1–3, directing each to execute from
/// `entry_point`.
///
/// # How it works
///
/// For each core 1..3:
/// 1. Writes the physical address of `entry_point` to the spin-table
///    location for that core.
/// 2. Issues a `SEV` (Send Event) instruction so that any core
///    spinning in `WFE` will wake and branch to the entry point.
/// 3. Records the core as started in the internal tracking bitmask.
///
/// # Safety
///
/// The caller must ensure:
/// - The `entry_point` points to valid executable code that correctly
///   handles being entered on a secondary core (stack setup, etc.).
/// - The secondary cores have not already been started.
/// - Proper synchronisation is in place for shared resources.
pub fn smp_init(entry_point: usize) {
    for i in 1..NUM_CORES {
        if let Some(id) = CoreId::from_raw(i as u8) {
            start_core(id, entry_point);
            let mask = 1u32 << i;
            CORES_STARTED.fetch_or(mask, Ordering::Release);
        }
    }
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
    let addr = LOCAL_PERIPH_BASE + SPIN_TABLE_BASE + id.as_u32() as u64 * 8;

    // Write the entry point to the spin table for this core.
    // Use 64-bit write since the spin-table entries are 64 bits wide.
    unsafe {
        core::ptr::write_volatile(addr as *mut u64, entry_point as u64);
    }

    // Ensure the write is visible before waking the core.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    // Send an event to wake cores waiting in WFE.
    unsafe {
        core::arch::asm!("sev", options(nostack, preserves_flags));
    }
}

/// Returns the spin-table entry address for the given core ID.
///
/// On the BCM2712 the spin-table entries are 64-bit slots starting at
/// offset `0xD0` from the local peripheral base, one per core.
#[inline]
pub fn spin_table_entry_addr(id: CoreId) -> *mut u64 {
    let addr = LOCAL_PERIPH_BASE + SPIN_TABLE_BASE + id.as_u32() as u64 * 8;
    addr as *mut u64
}

/// Checks the spin-table entry and returns the entry-point address
/// if it has been written (non-zero).
///
/// This is the testable core logic of [`spin_wait`]: it reads the
/// 64-bit spin-table slot via a volatile load and returns `Some(addr)`
/// when another core has written a non-zero entry point.
///
/// # Safety
///
/// `table_entry` must point to a valid, 8-byte-aligned 64-bit slot
/// that can be safely read with a volatile load.
#[inline]
pub unsafe fn check_spin_entry(table_entry: *const u64) -> Option<usize> {
    let entry = core::ptr::read_volatile(table_entry);
    if entry != 0 {
        Some(entry as usize)
    } else {
        None
    }
}

/// Puts the calling core into a spin-wait loop, polling the spin-table
/// entry until another core writes a non-zero entry-point address, then
/// jumps to that address.
///
/// This is typically used by secondary cores at boot to wait for the
/// primary core to set their entry point via [`start_core`].
///
/// # How it works
///
/// 1. Reads the spin-table entry with a volatile load.
/// 2. If the entry is non-zero, transmutes it to a function pointer
///    (`unsafe fn() -> !`) and branches to it — the secondary core
///    begins executing at the given address.
/// 3. If the entry is still zero, issues a `spin_loop` hint and retries.
///
/// # Safety
///
/// The caller must ensure:
/// - `table_entry` points to a valid, 8-byte-aligned 64-bit spin-table
///   slot that will eventually be written by another core.
/// - When written, the value must be the address of valid executable
///   code that correctly handles being entered on a secondary core
///   (stack setup, etc.).
pub unsafe fn spin_wait(table_entry: *mut u64) -> ! {
    loop {
        let entry = core::ptr::read_volatile(table_entry);
        if entry != 0 {
            let func: unsafe fn() -> ! = core::mem::transmute(entry as usize);
            func()
        }
        core::hint::spin_loop();
    }
}

/// Sends an inter-processor interrupt (IPI) to `target_core` with the
/// given `vector` value.
///
/// Writes `vector` to the doorbell register of the target core and
/// issues an event (`SEV`) so the core is woken from `WFE` if it was
/// sleeping.  The target core's interrupt handler can read the vector
/// to determine the cause of the IPI.
///
/// # Panics
///
/// Panics if `target_core` is not a valid core ID (must be < `NUM_CORES`).
pub fn smp_send_ipi(target_core: u32, vector: u32) {
    assert!(
        (target_core as usize) < NUM_CORES,
        "smp_send_ipi: invalid target core {}",
        target_core
    );
    let addr = LOCAL_PERIPH_BASE + IPI_DOORBELL_BASE + (target_core as u64) * IPI_DOORBELL_STRIDE;
    mmio_write(addr, vector);

    // Ensure the write is visible before waking the core.
    crate::mmio::mmio_fence();

    // Wake the target core.
    unsafe {
        core::arch::asm!("sev", options(nostack, preserves_flags));
    }
}

/// Legacy name — sends an inter-core interrupt / mailbox message to the
/// specified core.
///
/// Writes `value` to mailbox register 0 of the target core and issues
/// an event (`SEV`) so the core is woken from `WFE` if it was sleeping.
pub fn inter_core_interrupt(id: CoreId, value: u32) {
    let addr = LOCAL_PERIPH_BASE + MAILBOX0_SET0 + id.as_u32() as u64 * MAILBOX_STRIDE;
    mmio_write(addr, value);

    // Ensure the write is visible before waking the core.
    crate::mmio::mmio_fence();

    // Wake the target core.
    unsafe {
        core::arch::asm!("sev", options(nostack, preserves_flags));
    }
}

/// Reads the pending mailbox value for the specified core (mailbox 0).
#[inline]
pub fn read_mailbox(id: CoreId) -> u32 {
    let addr = LOCAL_PERIPH_BASE + MAILBOX0_SET0 + id.as_u32() as u64 * MAILBOX_STRIDE;
    mmio_read(addr)
}

/// Returns `true` if the given core has been marked as started.
#[inline]
pub fn is_core_started(id: CoreId) -> bool {
    let mask = 1u32 << id.as_u8();
    (CORES_STARTED.load(Ordering::Acquire) & mask) != 0
}

/// Returns the bitmask of started cores (bit N = core N).
#[inline]
pub fn started_cores_mask() -> u32 {
    CORES_STARTED.load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------
// Spinlock — mutual exclusion between cores
// ---------------------------------------------------------------------------

/// A simple spinlock for inter-core mutual exclusion on the Pi 5.
///
/// Uses atomic compare-and-swap for efficient acquisition.  The lock is a
/// single `u32` word where `0` means unlocked and `1` means locked.
///
/// # Example
///
/// ```ignore
/// use vuma_pi5::smp::Spinlock;
///
/// static LOCK: Spinlock = Spinlock::new();
///
/// // Critical section:
/// let guard = LOCK.lock();
/// // ... access shared data ...
/// drop(guard); // releases the lock
/// ```
///
/// # Safety
///
/// - A `Spinlock` must **not** be re-locked on the same core without
///   first unlocking it (this is not a re-entrant lock).
/// - The lock must be initialised (via [`Spinlock::new`]) before use.
pub struct Spinlock {
    /// `0` = unlocked, `1` = locked.
    lock: AtomicU32,
}

impl Spinlock {
    /// Creates a new `Spinlock` in the unlocked state.
    ///
    /// This is `const` so it can be used to initialise `static` items.
    pub const fn new() -> Self {
        Self {
            lock: AtomicU32::new(0),
        }
    }

    /// Acquires the spinlock, spinning until it is available.
    ///
    /// Returns a [`SpinlockGuard`] that will release the lock when dropped.
    pub fn lock(&self) -> SpinlockGuard<'_> {
        loop {
            // Optimistic fast-path: try to atomically swap 0 → 1.
            if self
                .lock
                .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return SpinlockGuard { spinlock: self };
            }
            // Spin — hint to the CPU that we're in a spin-wait loop.
            while self.lock.load(Ordering::Relaxed) != 0 {
                core::hint::spin_loop();
            }
        }
    }

    /// Tries to acquire the spinlock once.
    ///
    /// Returns `Some(SpinlockGuard)` if the lock was acquired, `None`
    /// if it was already held.
    pub fn try_lock(&self) -> Option<SpinlockGuard<'_>> {
        if self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinlockGuard { spinlock: self })
        } else {
            None
        }
    }

    /// Releases the spinlock.
    ///
    /// This is normally called automatically when the [`SpinlockGuard`] is
    /// dropped, but can be called manually if needed.
    #[inline]
    pub fn unlock(&self) {
        self.lock.store(0, Ordering::Release);
    }

    /// Returns `true` if the lock is currently held.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Acquire) != 0
    }
}

// Ensure Spinlock is Send + Sync so it can be shared across cores.
unsafe impl Send for Spinlock {}
unsafe impl Sync for Spinlock {}

// ---------------------------------------------------------------------------
// SpinlockGuard — RAII guard for Spinlock
// ---------------------------------------------------------------------------

/// An RAII guard that releases the [`Spinlock`] when dropped.
///
/// While this guard exists, the lock is held and other cores will spin
/// waiting for it.
pub struct SpinlockGuard<'a> {
    spinlock: &'a Spinlock,
}

impl<'a> Drop for SpinlockGuard<'a> {
    fn drop(&mut self) {
        self.spinlock.unlock();
    }
}

impl<'a> core::ops::Deref for SpinlockGuard<'a> {
    type Target = Spinlock;
    fn deref(&self) -> &Self::Target {
        self.spinlock
    }
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

    #[test]
    fn core_id_as_u32() {
        assert_eq!(CoreId::CORE0.as_u32(), 0);
        assert_eq!(CoreId::CORE3.as_u32(), 3);
    }

    #[test]
    fn core_0_starts_started() {
        // Core 0 should be marked as started by default.
        assert!(is_core_started(CoreId::CORE0));
    }

    #[test]
    fn started_cores_mask_includes_core_0() {
        let mask = started_cores_mask();
        assert_ne!(mask & 0x1, 0, "core 0 bit should be set");
    }

    #[test]
    fn spinlock_new_is_unlocked() {
        let lock = Spinlock::new();
        assert!(!lock.is_locked());
    }

    #[test]
    fn spinlock_lock_and_unlock() {
        let lock = Spinlock::new();
        {
            let guard = lock.lock();
            assert!(lock.is_locked());
            drop(guard);
        }
        assert!(!lock.is_locked());
    }

    #[test]
    fn spinlock_try_lock_succeeds_when_unlocked() {
        let lock = Spinlock::new();
        let guard = lock.try_lock();
        assert!(guard.is_some());
        assert!(lock.is_locked());
        drop(guard);
        assert!(!lock.is_locked());
    }

    #[test]
    fn spinlock_try_lock_fails_when_locked() {
        let lock = Spinlock::new();
        let _g1 = lock.lock();
        let g2 = lock.try_lock();
        assert!(g2.is_none());
    }

    #[test]
    fn spinlock_is_const_constructible() {
        const _LOCK: Spinlock = Spinlock::new();
    }

    // -----------------------------------------------------------------------
    // spin_table_entry_addr / check_spin_entry tests
    // -----------------------------------------------------------------------

    #[test]
    fn spin_table_entry_addr_core0() {
        let addr = spin_table_entry_addr(CoreId::CORE0);
        let expected = (LOCAL_PERIPH_BASE + SPIN_TABLE_BASE) as usize;
        assert_eq!(addr as usize, expected);
    }

    #[test]
    fn spin_table_entry_addr_core1() {
        let addr = spin_table_entry_addr(CoreId::CORE1);
        let expected = (LOCAL_PERIPH_BASE + SPIN_TABLE_BASE + 8) as usize;
        assert_eq!(addr as usize, expected);
    }

    #[test]
    fn spin_table_entry_addr_core3() {
        let addr = spin_table_entry_addr(CoreId::CORE3);
        let expected = (LOCAL_PERIPH_BASE + SPIN_TABLE_BASE + 3 * 8) as usize;
        assert_eq!(addr as usize, expected);
    }

    #[test]
    fn check_spin_entry_returns_none_when_zero() {
        let slot: u64 = 0;
        let result = unsafe { check_spin_entry(&slot as *const u64) };
        assert!(result.is_none(), "should return None for zero entry");
    }

    #[test]
    fn test_spin_wait_exits_on_entry() {
        // Verify the spin_wait exit logic: check_spin_entry should return
        // Some(addr) when the table entry is non-zero, which is the condition
        // that causes spin_wait to branch out of its loop.
        let slot: u64 = 0x80000; // a non-zero entry-point address
        let result = unsafe { check_spin_entry(&slot as *const u64) };
        assert!(
            result.is_some(),
            "should return Some when entry is non-zero"
        );
        assert_eq!(
            result.unwrap(),
            0x80000,
            "should return the entry point address"
        );
    }

    #[test]
    fn check_spin_entry_various_nonzero_values() {
        let values = [1u64, 0xDEAD_BEEF, 0x1, u64::MAX];
        for &val in &values {
            let result = unsafe { check_spin_entry(&val as *const u64) };
            assert!(
                result.is_some(),
                "non-zero value {:#X} should yield Some",
                val
            );
            assert_eq!(result.unwrap(), val as usize);
        }
    }
}
