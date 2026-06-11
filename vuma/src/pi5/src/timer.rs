//! ARM Generic Timer access for the Raspberry Pi 5.
//!
//! Provides both the [`Timer`] struct and free-standing convenience functions
//! backed by the AArch64 generic timer registers.  On the BCM2712
//! (Cortex-A76) the generic timer is accessible via system registers and does
//! not require MMIO.
//!
//! # Register overview
//!
//! | Register        | Purpose                                |
//! |-----------------|----------------------------------------|
//! | `CNTPCT_EL0`   | Physical counter (read-only)           |
//! | `CNTVCT_EL0`   | Virtual counter (read-only)            |
//! | `CNTFRQ_EL0`   | Counter frequency in Hz (firmware-set) |
//! | `CNTV_CTL_EL0` | Virtual timer control (enable / imask) |
//! | `CNTV_TVAL_EL0`| Virtual timer compare value (relative) |
//! | `CNTV_CVAL_EL0`| Virtual timer compare value (absolute) |
//!
//! # Free-standing API
//!
//! The module exposes the following free functions that delegate to a global
//! [`Timer`] instance, matching the C-style API commonly used in bare-metal
//! Pi 5 projects:
//!
//! - [`timer_init`]          — one-time initialisation
//! - [`timer_get_ticks`]     — read the virtual counter
//! - [`timer_get_micros`]    — microseconds since `timer_init`
//! - [`timer_delay_micros`]  — busy-wait delay
//! - [`timer_set_interval`]  — configure a periodic virtual-timer interrupt

use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Virtual timer control register bits (CNTV_CTL_EL0)
// ---------------------------------------------------------------------------

/// Bit 0 — Enable the virtual timer.
const CTL_ENABLE: u64 = 1 << 0;

/// Bit 1 — Interrupt mask (1 = masked / disabled).
const CTL_IMASK: u64 = 1 << 1;

/// Bit 2 — Timer condition status (read-only, 1 = timer fired).
const CTL_ISTATUS: u64 = 1 << 2;

// ---------------------------------------------------------------------------
// Global boot-time stamp (set by timer_init)
// ---------------------------------------------------------------------------

/// Ticks recorded at `timer_init()` time.  Initialised to 0 so that
/// `timer_get_micros()` returns sensible values even before `timer_init` is
/// called (it will simply report time since power-on).
static BOOT_TICKS: AtomicU64 = AtomicU64::new(0);

/// Whether `timer_init()` has been called.
static INITIALIZED: AtomicU64 = AtomicU64::new(0); // 0 = false, 1 = true

// ---------------------------------------------------------------------------
// Inline helpers — raw system-register access
// ---------------------------------------------------------------------------

/// Read the **physical** counter (`CNTPCT_EL0`).
#[inline(always)]
fn read_cntpct() -> u64 {
    let val: u64;
    // SAFETY: CNTPCT_EL0 is a readable system register on all AArch64
    // implementations that support the generic timer.
    unsafe {
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) val, options(nostack, preserves_flags));
    }
    val
}

/// Read the **virtual** counter (`CNTVCT_EL0`).
#[inline(always)]
fn read_cntvct() -> u64 {
    let val: u64;
    // SAFETY: CNTVCT_EL0 is a readable system register on AArch64
    // implementations with virtualisation support (all Cortex-A76).
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) val, options(nostack, preserves_flags));
    }
    val
}

/// Read the counter frequency (`CNTFRQ_EL0`) in Hz.
#[inline(always)]
fn read_cntfrq() -> u64 {
    let val: u64;
    // SAFETY: CNTFRQ_EL0 is a readable system register.
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) val, options(nostack, preserves_flags));
    }
    val
}

/// Read the virtual timer control register (`CNTV_CTL_EL0`).
#[inline(always)]
fn read_cntv_ctl() -> u64 {
    let val: u64;
    // SAFETY: CNTV_CTL_EL0 is a readable system register.
    unsafe {
        core::arch::asm!("mrs {}, cntv_ctl_el0", out(reg) val, options(nostack, preserves_flags));
    }
    val
}

/// Write the virtual timer control register (`CNTV_CTL_EL0`).
#[inline(always)]
fn write_cntv_ctl(val: u64) {
    // SAFETY: CNTV_CTL_EL0 is a writable system register.  We only toggle
    // the well-defined ENABLE / IMASK bits.
    unsafe {
        core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) val, options(nostack, preserves_flags));
    }
}

/// Write the virtual timer compare value — relative (`CNTV_TVAL_EL0`).
///
/// Writing a value `N` to TVAL causes an interrupt after `N` ticks of the
/// virtual counter have elapsed.
#[inline(always)]
fn write_cntv_tval(val: u64) {
    // SAFETY: CNTV_TVAL_EL0 is a writable system register.
    unsafe {
        core::arch::asm!("msr cntv_tval_el0, {}", in(reg) val, options(nostack, preserves_flags));
    }
}

/// Write the virtual timer compare value — absolute (`CNTV_CVAL_EL0`).
///
/// An interrupt fires when the virtual counter reaches `val`.
#[inline(always)]
fn write_cntv_cval(val: u64) {
    // SAFETY: CNTV_CVAL_EL0 is a writable system register.
    unsafe {
        core::arch::asm!("msr cntv_cval_el0, {}", in(reg) val, options(nostack, preserves_flags));
    }
}

// ---------------------------------------------------------------------------
// Timer struct
// ---------------------------------------------------------------------------

/// A handle to the ARM Generic Timer.
///
/// On the BCM2712 (Cortex-A76) the generic timer is accessible via
/// system registers and does not require a base address.
#[derive(Debug, Clone, Copy)]
pub struct Timer;

impl Timer {
    /// Creates a new `Timer` handle.
    ///
    /// There is no per-instance state; the handle exists for API
    /// consistency with other subsystems.
    #[inline]
    pub const fn new() -> Self {
        Self
    }

    // -----------------------------------------------------------------------
    // Counter reads
    // -----------------------------------------------------------------------

    /// Reads the current **physical** counter value from `CNTPCT_EL0`.
    ///
    /// The counter increments at the frequency reported by [`frequency`].
    #[inline(always)]
    pub fn current_ticks(&self) -> u64 {
        read_cntpct()
    }

    /// Reads the current **virtual** counter value from `CNTVCT_EL0`.
    ///
    /// In EL1 without virtualisation the virtual counter equals the physical
    /// counter, but this is the preferred register for guest / VM code.
    #[inline(always)]
    pub fn virtual_ticks(&self) -> u64 {
        read_cntvct()
    }

    /// Reads the timer frequency from `CNTFRQ_EL0` (in Hz).
    ///
    /// On the Pi 5 this is typically 54 MHz, set by the firmware.
    #[inline(always)]
    pub fn frequency(&self) -> u64 {
        read_cntfrq()
    }

    // -----------------------------------------------------------------------
    // Conversion helpers
    // -----------------------------------------------------------------------

    /// Converts a tick count to nanoseconds.
    ///
    /// Uses 64-bit arithmetic to avoid overflow for large tick values.
    #[inline]
    pub fn ticks_to_ns(&self, ticks: u64) -> u64 {
        let freq = self.frequency();
        // (ticks / freq) * 1e9 + (ticks % freq) * 1e9 / freq
        let secs = ticks / freq;
        let sub = ticks % freq;
        secs * 1_000_000_000 + (sub * 1_000_000_000) / freq
    }

    /// Converts a tick count to microseconds.
    #[inline]
    pub fn ticks_to_us(&self, ticks: u64) -> u64 {
        let freq = self.frequency();
        let secs = ticks / freq;
        let sub = ticks % freq;
        secs * 1_000_000 + (sub * 1_000_000) / freq
    }

    /// Converts a tick count to milliseconds.
    #[inline]
    pub fn ticks_to_ms(&self, ticks: u64) -> u64 {
        let freq = self.frequency();
        let secs = ticks / freq;
        let sub = ticks % freq;
        secs * 1_000 + (sub * 1_000) / freq
    }

    /// Converts a microsecond duration to ticks.
    #[inline]
    pub fn us_to_ticks(&self, us: u64) -> u64 {
        let freq = self.frequency();
        // us * freq / 1_000_000  — rearranged for overflow avoidance
        let whole = (us / 1_000_000) * freq;
        let sub = (us % 1_000_000) * freq;
        whole + sub / 1_000_000
    }

    // -----------------------------------------------------------------------
    // Busy-wait delays
    // -----------------------------------------------------------------------

    /// Busy-waits for the specified number of nanoseconds.
    ///
    /// Granularity is limited by the timer frequency (~18.5 ns at 54 MHz).
    pub fn delay_ns(&self, ns: u64) {
        let freq = self.frequency();
        // target_ticks = ns * freq / 1_000_000_000
        let target = (ns / 1_000_000_000) * freq + ((ns % 1_000_000_000) * freq) / 1_000_000_000;
        let start = self.current_ticks();
        while self.current_ticks().wrapping_sub(start) < target {
            core::hint::spin_loop();
        }
    }

    /// Busy-waits for the specified number of microseconds.
    pub fn delay_us(&self, us: u64) {
        self.delay_ns(us * 1_000);
    }

    /// Busy-waits for the specified number of milliseconds.
    pub fn delay_ms(&self, ms: u64) {
        self.delay_ns(ms * 1_000_000);
    }

    // -----------------------------------------------------------------------
    // Virtual timer control (CNTV_CTL_EL0 / CNTV_TVAL_EL0)
    // -----------------------------------------------------------------------

    /// Disables the virtual timer and masks its interrupt.
    ///
    /// Clears the ENABLE bit and sets the IMASK bit in `CNTV_CTL_EL0`.
    #[inline]
    pub fn virtual_timer_disable(&self) {
        write_cntv_ctl(CTL_IMASK); // disabled + masked
    }

    /// Enables the virtual timer and un-masks its interrupt.
    ///
    /// Sets the ENABLE bit and clears the IMASK bit in `CNTV_CTL_EL0`.
    #[inline]
    pub fn virtual_timer_enable(&self) {
        write_cntv_ctl(CTL_ENABLE); // enabled + unmasked
    }

    /// Returns `true` if the virtual timer condition has been met (timer
    /// has fired) by reading the ISTATUS bit of `CNTV_CTL_EL0`.
    #[inline]
    pub fn virtual_timer_fired(&self) -> bool {
        (read_cntv_ctl() & CTL_ISTATUS) != 0
    }

    /// Sets a relative interval on the virtual timer using `CNTV_TVAL_EL0`
    /// and enables the timer interrupt.
    ///
    /// After `micros` microseconds the virtual timer will fire.  The caller
    /// must handle the interrupt in EL1 and re-arm the timer if a periodic
    /// interrupt is desired.
    pub fn set_virtual_timer_interval(&self, micros: u64) {
        let ticks = self.us_to_ticks(micros);
        // Disable + mask while configuring.
        self.virtual_timer_disable();
        // Set the relative compare value.
        write_cntv_tval(ticks);
        // Enable + unmask → interrupt will fire when the counter reaches
        // (current_virtual_ticks + ticks).
        self.virtual_timer_enable();
    }

    /// Sets an absolute deadline on the virtual timer using `CNTV_CVAL_EL0`
    /// and enables the timer interrupt.
    ///
    /// The timer fires when the virtual counter reaches `deadline_ticks`.
    pub fn set_virtual_timer_deadline(&self, deadline_ticks: u64) {
        self.virtual_timer_disable();
        write_cntv_cval(deadline_ticks);
        self.virtual_timer_enable();
    }

    // -----------------------------------------------------------------------
    // Init / boot-time support
    // -----------------------------------------------------------------------

    /// Performs one-time initialisation of the timer subsystem.
    ///
    /// 1. Records the current virtual counter as the boot time-stamp.
    /// 2. Disables the virtual timer interrupt (it can be re-enabled later
    ///    via [`set_virtual_timer_interval`](Self::set_virtual_timer_interval)).
    ///
    /// This is safe to call more than once — subsequent calls are no-ops.
    pub fn init(&self) {
        if INITIALIZED.load(Ordering::Acquire) != 0 {
            return;
        }
        let now = self.virtual_ticks();
        BOOT_TICKS.store(now, Ordering::Release);
        // Disable the virtual timer and mask its interrupt until the
        // kernel explicitly sets an interval.
        self.virtual_timer_disable();
        INITIALIZED.store(1, Ordering::Release);
    }

    /// Returns the tick value recorded at `init()` time.
    #[inline]
    pub fn boot_ticks(&self) -> u64 {
        BOOT_TICKS.load(Ordering::Acquire)
    }

    /// Returns the number of microseconds elapsed since `init()`.
    #[inline]
    pub fn micros_since_boot(&self) -> u64 {
        let now = self.virtual_ticks();
        let boot = BOOT_TICKS.load(Ordering::Acquire);
        let elapsed = now.wrapping_sub(boot);
        self.ticks_to_us(elapsed)
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Free-standing convenience API
// ---------------------------------------------------------------------------

/// One-time initialisation of the timer subsystem.
///
/// Delegates to [`Timer::init`].  Safe to call multiple times.
pub fn timer_init() {
    Timer::new().init();
}

/// Reads the current virtual counter value (`CNTVCT_EL0`).
///
/// This is the preferred counter for bare-metal Pi 5 code because it is
/// subject to virtual-offset adjustments when running under a hypervisor.
#[inline(always)]
pub fn timer_get_ticks() -> u64 {
    read_cntvct()
}

/// Returns microseconds elapsed since [`timer_init`] was called.
///
/// If `timer_init` has not been called, the value is relative to the
/// power-on counter value (effectively time since boot).
#[inline]
pub fn timer_get_micros() -> u64 {
    Timer::new().micros_since_boot()
}

/// Busy-waits for the specified number of microseconds.
///
/// Uses the physical counter (`CNTPCT_EL0`) for the busy-wait loop.
pub fn timer_delay_micros(us: u64) {
    Timer::new().delay_us(us);
}

/// Configures the virtual timer to fire an interrupt every `micros`
/// microseconds and enables the timer.
///
/// The interrupt handler must acknowledge the timer (typically by
/// re-writing `CNTV_TVAL_EL0` with the same interval and clearing the
/// pending interrupt status) to achieve a truly periodic interrupt.
pub fn timer_set_interval(micros: u64) {
    Timer::new().set_virtual_timer_interval(micros);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_is_default_constructible() {
        let _t = Timer::default();
    }

    #[test]
    fn timer_new_is_const() {
        const _T: Timer = Timer::new();
    }

    #[test]
    fn ticks_to_us_round_trip_identity() {
        let t = Timer::new();
        let freq = t.frequency();
        if freq == 0 {
            // Cannot perform the test without a valid frequency.
            return;
        }
        // 1 second in ticks → should be exactly 1_000_000 us.
        let one_sec_ticks = freq;
        assert_eq!(t.ticks_to_us(one_sec_ticks), 1_000_000);
    }

    #[test]
    fn ticks_to_ms_one_second() {
        let t = Timer::new();
        let freq = t.frequency();
        if freq == 0 {
            return;
        }
        assert_eq!(t.ticks_to_ms(freq), 1_000);
    }

    #[test]
    fn ticks_to_ns_one_second() {
        let t = Timer::new();
        let freq = t.frequency();
        if freq == 0 {
            return;
        }
        assert_eq!(t.ticks_to_ns(freq), 1_000_000_000);
    }

    #[test]
    fn us_to_ticks_round_trip() {
        let t = Timer::new();
        let freq = t.frequency();
        if freq == 0 {
            return;
        }
        // 1_000_000 us (1 second) → should equal the frequency.
        assert_eq!(t.us_to_ticks(1_000_000), freq);
    }

    #[test]
    fn virtual_timer_ctl_constants_non_overlapping() {
        // Ensure the CTL bit constants are distinct.
        assert_eq!(CTL_ENABLE & CTL_IMASK, 0);
        assert_eq!(CTL_ENABLE & CTL_ISTATUS, 0);
        assert_eq!(CTL_IMASK & CTL_ISTATUS, 0);
    }

    #[test]
    fn boot_ticks_initially_zero_before_init() {
        // We haven't called timer_init in this test, so BOOT_TICKS
        // should still be 0 (or whatever the default is).
        assert_eq!(BOOT_TICKS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn initialized_flag_starts_false() {
        assert_eq!(INITIALIZED.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn free_standing_api_compiles() {
        // Ensure the free functions are accessible and have correct signatures.
        let _ticks: u64 = timer_get_ticks();
        let _micros: u64 = timer_get_micros();
        // We do NOT call timer_init or timer_set_interval here because they
        // have side effects on the timer hardware, but we verify they exist.
    }
}
