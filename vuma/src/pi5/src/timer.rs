//! ARM Generic Timer access for the Raspberry Pi 5.
//!
//! Provides the [`Timer`] struct backed by the AArch64 generic timer
//! registers (`CNTPCT_EL0` and `CNTFRQ_EL0`). These are per-core
//! registers provided by the Cortex-A76 and do not require MMIO.

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

    /// Reads the current physical counter value from `CNTPCT_EL0`.
    ///
    /// The counter increments at the frequency reported by [`frequency`].
    #[inline(always)]
    pub fn current_ticks(&self) -> u64 {
        let ticks: u64;
        // SAFETY: CNTPCT_EL0 is a readable system register on all
        // AArch64 implementations that support the generic timer.
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) ticks, options(nostack, preserves_flags));
        ticks
    }

    /// Reads the timer frequency from `CNTFRQ_EL0` (in Hz).
    ///
    /// On the Pi 5 this is typically 54 MHz, set by the firmware.
    #[inline(always)]
    pub fn frequency(&self) -> u64 {
        let freq: u64;
        // SAFETY: CNTFRQ_EL0 is a readable system register.
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nostack, preserves_flags));
        freq
    }

    /// Converts a tick count to nanoseconds.
    ///
    /// Uses 64-bit arithmetic to avoid overflow for large tick values.
    #[inline]
    pub fn ticks_to_ns(&self, ticks: u64) -> u64 {
        let freq = self.frequency();
        // ticks * 1_000_000_000 / freq  — rearranged to avoid overflow
        // where possible: (ticks / freq) * 1e9 + (ticks % freq) * 1e9 / freq
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

    /// Busy-waits for the specified number of nanoseconds.
    ///
    /// Granularity is limited by the timer frequency (~18.5 ns at 54 MHz).
    pub fn delay_ns(&self, ns: u64) {
        let freq = self.frequency();
        // target_ticks = ns * freq / 1_000_000_000
        let target = (ns / 1_000_000_000) * freq
            + ((ns % 1_000_000_000) * freq) / 1_000_000_000;
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
}

impl Default for Timer {
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
    fn timer_is_default_constructible() {
        let _t = Timer::default();
    }
}
