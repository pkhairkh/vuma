//! Raspberry Pi 5 platform description and constants.
//!
//! Defines the [`Pi5Platform`] struct implementing a [`Platform`] trait with
//! hardware-specific constants for the BCM2712 SoC found in the Pi 5,
//! including cache parameters, memory map, and peripheral base addresses.
//!
//! # Pi 5 Architecture
//!
//! The Pi 5 uses the BCM2712 SoC with an external **RP1 I/O co-processor**
//! connected via PCIe. GPIO and PWM peripherals are accessed through the RP1,
//! **not** through the legacy BCM2835-style peripheral address space.
//!
//! | Peripheral | Address        | Notes                             |
//! |------------|----------------|-----------------------------------|
//! | RP1 GPIO   | `0x1F00010000` | GPIO via RP1 I/O chip             |
//! | RP1 PWM0   | `0x1F00014000` | PWM channel 0 via RP1             |
//! | RP1 PWM1   | `0x1F00015000` | PWM channel 1 via RP1             |
//! | BCM UART   | `0x1C00101000` | PL011 UART (legacy peripheral)    |

use serde::{Deserialize, Serialize};

/// Number of Cortex-A76 cores on the BCM2712.
pub const NUM_CORES: usize = 4;

/// L1 Data cache size in bytes (64 KiB per core on Cortex-A76).
pub const L1D_CACHE_SIZE: usize = 65536;

/// L1 Data cache line size in bytes.
pub const L1D_CACHE_LINE: usize = 64;

/// L2 Cache size in bytes (256 KiB per core on Cortex-A76).
pub const L2_CACHE_SIZE: usize = 262144;

/// L3 Shared cache size in bytes (2 MiB shared across all cores).
pub const L3_CACHE_SIZE: usize = 2097152;

/// DRAM base address — starts at physical 0 on the Pi 5.
pub const RAM_BASE: u64 = 0x0000_0000;

/// BCM2712 peripheral register space start (Pi 5 native address).
pub const BCM2712_PERIPHERAL_START: u64 = 0x0010_0000;
/// BCM2712 peripheral register space end (inclusive).
pub const BCM2712_PERIPHERAL_END: u64 = 0x001F_FFFF;

/// RP1 I/O co-processor register space start.
pub const RP1_IO_START: u64 = 0x1F_0001_0000;
/// RP1 I/O co-processor register space end (inclusive).
pub const RP1_IO_END: u64 = 0x1F_0001_FFFF;

/// ARM local (per-core) register space start.
pub const ARM_LOCAL_START: u64 = 0x7C00_0000_0000;
/// ARM local (per-core) register space end (inclusive).
pub const ARM_LOCAL_END: u64 = 0x7CFF_FFFF_FFFF;

/// Peripheral base address (low-peripheral mode, legacy alias).
pub const PERIPHERAL_BASE: u64 = 0x1c00_0000;

/// Peripheral base address (high-peripheral mode).
pub const PERIPHERAL_BASE_HIGH: u64 = 0x7c00_0000;

/// Default RAM size assumed for Pi 5 (4 GiB variant).
pub const DEFAULT_RAM_SIZE: u64 = 4 * 1024 * 1024 * 1024;

/// Maximum supported RAM size on Pi 5 (8 GiB).
pub const MAX_RAM_SIZE: u64 = 8 * 1024 * 1024 * 1024;

// ---------------------------------------------------------------------------
// RP1 peripheral base addresses
// ---------------------------------------------------------------------------

/// RP1 chip base address as seen by the ARM cores (via PCIe).
pub const RP1_BASE: u64 = 0x1F_0000_0000;

/// RP1 GPIO controller base address.
///
/// On the Pi 5, GPIO is accessed through the RP1 I/O co-processor at this
/// physical address, **not** through the legacy BCM2835-style registers at
/// `0x1C000000`.
pub const RP1_GPIO_BASE: u64 = 0x1F_0001_0000;

/// RP1 PWM channel 0 base address.
///
/// PWM0 is available on GPIO12 (AltFunc0) and GPIO18 (AltFunc5).
pub const RP1_PWM0_BASE: u64 = 0x1F_0001_4000;

/// RP1 PWM channel 1 base address.
///
/// PWM1 is available on GPIO13 (AltFunc0) and GPIO19 (AltFunc5).
pub const RP1_PWM1_BASE: u64 = 0x1F_0001_5000;

// ---------------------------------------------------------------------------
// Legacy peripheral offsets (relative to PERIPHERAL_BASE)
// ---------------------------------------------------------------------------

/// GPIO controller offset from peripheral base (legacy, not used on Pi 5).
pub const GPIO_BASE_OFFSET: u64 = 0x0000_0000;

/// PL011 UART0 offset from peripheral base.
/// On the BCM2712 (Pi 5) the primary PL011 is at physical address 0x10A0000.
pub const UART_BASE_OFFSET: u64 = 0x010A_0000;

/// AUX peripheral block offset from peripheral base.
/// On the BCM2712 the AUX block (containing mini UART / UART1) is at
/// physical address 0x10A8000.
pub const AUX_BASE_OFFSET: u64 = 0x010A_8000;

/// SPI (SPI0) offset from peripheral base.
pub const SPI_BASE_OFFSET: u64 = 0x0020_4000;

/// BSC (I2C) offset from peripheral base.
pub const I2C_BASE_OFFSET: u64 = 0x0020_5000;

/// PCIe controller offset from peripheral base.
pub const PCIE_BASE_OFFSET: u64 = 0x0150_0000;

/// DMA controller offset from peripheral base.
pub const DMA_BASE_OFFSET: u64 = 0x0000_7000;

// ---------------------------------------------------------------------------
// Platform trait
// ---------------------------------------------------------------------------

/// A trait describing the hardware platform for a bare-metal target.
///
/// Consumers of `vuma-pi5` should rely on this trait so that platform
/// specifics can be swapped in tests or for alternative SoCs.
pub trait Platform {
    /// Returns the cache-line size in bytes.
    fn cache_line_size(&self) -> usize;

    /// Returns the number of CPU cores available.
    fn num_cores(&self) -> usize;

    /// Returns the base address of the peripheral address space.
    fn peripheral_base(&self) -> u64;

    /// Returns the total size of installed RAM in bytes.
    fn ram_size(&self) -> u64;
}

// ---------------------------------------------------------------------------
// Pi5Platform
// ---------------------------------------------------------------------------

/// Whether the Pi 5 is operating in low- or high-peripheral mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeripheralMode {
    /// Peripherals mapped at `0x1c000000`.
    Low,
    /// Peripherals mapped at `0x7c000000`.
    High,
}

impl Default for PeripheralMode {
    fn default() -> Self {
        // The Pi 5 firmware typically uses low-peripheral mode.
        PeripheralMode::Low
    }
}

/// Platform descriptor for the Raspberry Pi 5 (BCM2712).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pi5Platform {
    /// Peripheral mapping mode (low or high).
    pub peripheral_mode: PeripheralMode,
    /// Total installed RAM in bytes. Defaults to 4 GiB.
    pub ram_size: u64,
}

impl Pi5Platform {
    /// Creates a new `Pi5Platform` with the given peripheral mode and RAM size.
    pub const fn new(peripheral_mode: PeripheralMode, ram_size: u64) -> Self {
        Self {
            peripheral_mode,
            ram_size,
        }
    }

    /// Creates a platform descriptor with default settings:
    /// low-peripheral mode and 4 GiB RAM.
    pub const fn default_platform() -> Self {
        Self::new(PeripheralMode::Low, DEFAULT_RAM_SIZE)
    }

    // -----------------------------------------------------------------------
    // RP1 peripheral addresses (Pi 5 specific)
    // -----------------------------------------------------------------------

    /// Returns the absolute base address of the RP1 GPIO controller.
    ///
    /// On the Pi 5, GPIO is accessed through the RP1 I/O co-processor,
    /// not through the legacy BCM2835-style registers.
    #[inline]
    pub fn rp1_gpio_base(&self) -> u64 {
        RP1_GPIO_BASE
    }

    /// Returns the absolute base address of RP1 PWM channel 0.
    #[inline]
    pub fn rp1_pwm0_base(&self) -> u64 {
        RP1_PWM0_BASE
    }

    /// Returns the absolute base address of RP1 PWM channel 1.
    #[inline]
    pub fn rp1_pwm1_base(&self) -> u64 {
        RP1_PWM1_BASE
    }

    // -----------------------------------------------------------------------
    // Legacy BCM peripheral addresses
    // -----------------------------------------------------------------------

    /// Returns the absolute base address for the legacy GPIO controller.
    ///
    /// **Note:** On the Pi 5, use [`rp1_gpio_base`](Self::rp1_gpio_base)
    /// instead — GPIO is on the RP1, not at this legacy address.
    #[inline]
    pub fn gpio_base(&self) -> u64 {
        self.peripheral_base() + GPIO_BASE_OFFSET
    }

    /// Returns the absolute base address for the PL011 UART (UART0).
    #[inline]
    pub fn uart_base(&self) -> u64 {
        self.peripheral_base() + UART_BASE_OFFSET
    }

    /// Returns the absolute base address for the AUX peripheral block
    /// (mini UART / UART1).
    #[inline]
    pub fn aux_base(&self) -> u64 {
        self.peripheral_base() + AUX_BASE_OFFSET
    }

    /// Returns the absolute base address for SPI0.
    #[inline]
    pub fn spi_base(&self) -> u64 {
        self.peripheral_base() + SPI_BASE_OFFSET
    }

    /// Returns the absolute base address for the BSC (I2C) controller.
    #[inline]
    pub fn i2c_base(&self) -> u64 {
        self.peripheral_base() + I2C_BASE_OFFSET
    }

    /// Returns the absolute base address for the PCIe controller.
    #[inline]
    pub fn pcie_base(&self) -> u64 {
        self.peripheral_base() + PCIE_BASE_OFFSET
    }

    /// Returns the absolute base address for the DMA controller.
    #[inline]
    pub fn dma_base(&self) -> u64 {
        self.peripheral_base() + DMA_BASE_OFFSET
    }
}

impl Default for Pi5Platform {
    fn default() -> Self {
        Self::default_platform()
    }
}

impl Platform for Pi5Platform {
    fn cache_line_size(&self) -> usize {
        L1D_CACHE_LINE
    }

    fn num_cores(&self) -> usize {
        NUM_CORES
    }

    fn peripheral_base(&self) -> u64 {
        match self.peripheral_mode {
            PeripheralMode::Low => PERIPHERAL_BASE,
            PeripheralMode::High => PERIPHERAL_BASE_HIGH,
        }
    }

    fn ram_size(&self) -> u64 {
        self.ram_size
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_platform_uses_low_peripheral_mode() {
        let p = Pi5Platform::default();
        assert_eq!(p.peripheral_base(), PERIPHERAL_BASE);
    }

    #[test]
    fn high_peripheral_mode_base() {
        let p = Pi5Platform::new(PeripheralMode::High, DEFAULT_RAM_SIZE);
        assert_eq!(p.peripheral_base(), PERIPHERAL_BASE_HIGH);
    }

    #[test]
    fn platform_trait_impl() {
        let p = Pi5Platform::default();
        assert_eq!(p.cache_line_size(), L1D_CACHE_LINE);
        assert_eq!(p.num_cores(), NUM_CORES);
        assert_eq!(p.ram_size(), DEFAULT_RAM_SIZE);
    }

    #[test]
    fn gpio_base_is_peripheral_base_plus_offset() {
        let p = Pi5Platform::default();
        assert_eq!(p.gpio_base(), PERIPHERAL_BASE + GPIO_BASE_OFFSET);
    }

    #[test]
    fn rp1_gpio_base_address() {
        let p = Pi5Platform::default();
        assert_eq!(p.rp1_gpio_base(), RP1_GPIO_BASE);
        assert_eq!(p.rp1_gpio_base(), 0x1F_0001_0000);
    }

    #[test]
    fn rp1_pwm_base_addresses() {
        let p = Pi5Platform::default();
        assert_eq!(p.rp1_pwm0_base(), RP1_PWM0_BASE);
        assert_eq!(p.rp1_pwm1_base(), RP1_PWM1_BASE);
    }
}
