//! Raspberry Pi 5 platform description and constants.
//!
//! Defines the [`Pi5Platform`] struct implementing a [`Platform`] trait with
//! hardware-specific constants for the BCM2712 SoC found in the Pi 5,
//! including cache parameters, memory map, and peripheral base addresses.

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
pub const RAM_BASE: usize = 0x0000_0000;

/// Peripheral base address (low-peripheral mode).
pub const PERIPHERAL_BASE: usize = 0x1c00_0000;

/// Peripheral base address (high-peripheral mode).
pub const PERIPHERAL_BASE_HIGH: usize = 0x7c00_0000;

/// Default RAM size assumed for Pi 5 (4 GiB variant).
pub const DEFAULT_RAM_SIZE: usize = 4 * 1024 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Peripheral offsets (relative to PERIPHERAL_BASE)
// ---------------------------------------------------------------------------

/// GPIO controller offset from peripheral base.
pub const GPIO_BASE_OFFSET: usize = 0x0000_0000;

/// PL011 UART offset from peripheral base.
pub const UART_BASE_OFFSET: usize = 0x0010_1000;

/// SPI (SPI0) offset from peripheral base.
pub const SPI_BASE_OFFSET: usize = 0x0020_4000;

/// BSC (I2C) offset from peripheral base.
pub const I2C_BASE_OFFSET: usize = 0x0020_5000;

/// PCIe controller offset from peripheral base.
pub const PCIE_BASE_OFFSET: usize = 0x0150_0000;

/// DMA controller offset from peripheral base.
pub const DMA_BASE_OFFSET: usize = 0x0000_7000;

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
    fn peripheral_base(&self) -> usize;

    /// Returns the total size of installed RAM in bytes.
    fn ram_size(&self) -> usize;
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
    pub ram_size: usize,
}

impl Pi5Platform {
    /// Creates a new `Pi5Platform` with the given peripheral mode and RAM size.
    pub const fn new(peripheral_mode: PeripheralMode, ram_size: usize) -> Self {
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

    /// Returns the absolute base address for the GPIO controller.
    #[inline]
    pub fn gpio_base(&self) -> usize {
        self.peripheral_base() + GPIO_BASE_OFFSET
    }

    /// Returns the absolute base address for the PL011 UART.
    #[inline]
    pub fn uart_base(&self) -> usize {
        self.peripheral_base() + UART_BASE_OFFSET
    }

    /// Returns the absolute base address for SPI0.
    #[inline]
    pub fn spi_base(&self) -> usize {
        self.peripheral_base() + SPI_BASE_OFFSET
    }

    /// Returns the absolute base address for the BSC (I2C) controller.
    #[inline]
    pub fn i2c_base(&self) -> usize {
        self.peripheral_base() + I2C_BASE_OFFSET
    }

    /// Returns the absolute base address for the PCIe controller.
    #[inline]
    pub fn pcie_base(&self) -> usize {
        self.peripheral_base() + PCIE_BASE_OFFSET
    }

    /// Returns the absolute base address for the DMA controller.
    #[inline]
    pub fn dma_base(&self) -> usize {
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

    fn peripheral_base(&self) -> usize {
        match self.peripheral_mode {
            PeripheralMode::Low => PERIPHERAL_BASE,
            PeripheralMode::High => PERIPHERAL_BASE_HIGH,
        }
    }

    fn ram_size(&self) -> usize {
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
}
