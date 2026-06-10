//! Runtime configuration for the Continuous Optimization Runtime (COR).
//!
//! This module defines the [`Config`] struct and [`TargetArch`] enum, which
//! control the behavior of the COR runtime including optimization aggressiveness,
//! verification time budgets, speculative optimization enablement, and the
//! target architecture for code generation.

use serde::{Deserialize, Serialize};

/// Target architecture for compiled code generation.
///
/// The runtime generates machine code (or IR) tailored to a specific
/// instruction set architecture. The choice of target influences register
/// allocation, instruction selection, and vectorization strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetArch {
    /// x86-64 (AMD64) – general-purpose workstations and servers.
    X86_64,
    /// AArch64 – 64-bit ARM, common in cloud instances (Graviton, etc.).
    AArch64,
    /// ARMv8-A – 32-bit/64-bit ARM with v8-A extensions, used on Raspberry Pi 5
    /// and similar embedded-class boards.
    ArmV8A,
}

impl Default for TargetArch {
    fn default() -> Self {
        // Most development happens on x86-64; production may target AArch64 or Pi 5.
        #[cfg(target_arch = "x86_64")]
        return TargetArch::X86_64;

        #[cfg(target_arch = "aarch64")]
        return TargetArch::AArch64;

        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        return TargetArch::ArmV8A;
    }
}

/// Optimization aggressiveness level.
///
/// Controls how aggressively the runtime optimizes compiled regions.
/// Higher levels trade longer compilation time for better runtime performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OptimizationLevel {
    /// Minimal optimization – fast compilation, useful for debugging.
    None,
    /// Basic optimizations – constant folding, dead code elimination.
    #[default]
    Basic,
    /// Aggressive optimizations – inlining, vectorization, loop transformations.
    Aggressive,
}

/// COR runtime configuration.
///
/// Encapsulates all tunable parameters that govern how the Continuous
/// Optimization Runtime behaves. Configuration is typically loaded once at
/// startup and shared immutably across the runtime.
///
/// # Example
///
/// ```
/// use vuma_cor::config::Config;
///
/// let config = Config::default();
/// assert!(config.enable_speculative);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Optimization aggressiveness level.
    pub optimization_level: OptimizationLevel,

    /// Maximum wall-clock time (in milliseconds) allowed for a single
    /// verification pass before the runtime must yield. Prevents
    /// verification from starving execution under pathological inputs.
    pub max_verification_time_ms: u64,

    /// Whether speculative optimization is enabled.
    ///
    /// When `true`, the runtime may speculatively optimize based on
    /// profile-guided assumptions (e.g. likely branch directions, hot
    /// paths). If an assumption is later invalidated, the runtime
    /// deoptimizes back to the safe fallback.
    pub enable_speculative: bool,

    /// Target architecture for code generation.
    pub target_arch: TargetArch,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            optimization_level: OptimizationLevel::default(),
            max_verification_time_ms: 100,
            enable_speculative: true,
            target_arch: TargetArch::default(),
        }
    }
}

impl Config {
    /// Creates a new configuration with the given optimization level.
    pub fn with_optimization_level(mut self, level: OptimizationLevel) -> Self {
        self.optimization_level = level;
        self
    }

    /// Creates a new configuration with the given verification time budget.
    ///
    /// # Panics
    ///
    /// Panics if `ms` is 0, which would mean no verification time is allowed.
    pub fn with_max_verification_time_ms(mut self, ms: u64) -> Self {
        assert!(ms > 0, "max_verification_time_ms must be > 0");
        self.max_verification_time_ms = ms;
        self
    }

    /// Creates a new configuration with speculative optimization enabled or disabled.
    pub fn with_speculative(mut self, enable: bool) -> Self {
        self.enable_speculative = enable;
        self
    }

    /// Creates a new configuration targeting the given architecture.
    pub fn with_target_arch(mut self, arch: TargetArch) -> Self {
        self.target_arch = arch;
        self
    }

    /// Returns `true` if the runtime should target a Raspberry Pi 5 board.
    pub fn is_pi5_target(&self) -> bool {
        matches!(self.target_arch, TargetArch::ArmV8A)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_sensible() {
        let cfg = Config::default();
        assert!(cfg.enable_speculative);
        assert_eq!(cfg.max_verification_time_ms, 100);
    }

    #[test]
    fn builder_pattern_works() {
        let cfg = Config::default()
            .with_optimization_level(OptimizationLevel::Aggressive)
            .with_speculative(false)
            .with_target_arch(TargetArch::ArmV8A);

        assert!(cfg.is_pi5_target());
        assert!(!cfg.enable_speculative);
        assert_eq!(cfg.optimization_level, OptimizationLevel::Aggressive);
    }
}
