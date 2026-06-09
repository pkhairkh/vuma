//! GPIO (General-Purpose Input/Output) access for the Raspberry Pi 5.
//!
//! Provides the [`GpioPin`] struct and associated enums for configuring,
//! reading, and writing individual GPIO pins. Register layout follows the
//! BCM2712 GPIO controller, which is register-compatible with the classic
//! BCM2835-style GPIO block.

use crate::mmio::{mmio_read, mmio_write, Address};

// ---------------------------------------------------------------------------
// GPIO register offsets (relative to GPIO_BASE)
// ---------------------------------------------------------------------------

/// GPIO Function Select registers (GPFSEL0 – GPFSEL5).
/// Each register controls 10 pins, 3 bits per pin.
pub const GPFSEL0: usize = 0x00;
pub const GPFSEL1: usize = 0x04;
pub const GPFSEL2: usize = 0x08;
pub const GPFSEL3: usize = 0x0C;
pub const GPFSEL4: usize = 0x10;
pub const GPFSEL5: usize = 0x14;

/// GPIO Pin Output Set registers (GPSET0 – GPSET1).
/// Write 1 to a bit to set the corresponding pin high.
pub const GPSET0: usize = 0x1C;
pub const GPSET1: usize = 0x20;

/// GPIO Pin Output Clear registers (GPCLR0 – GPCLR1).
/// Write 1 to a bit to set the corresponding pin low.
pub const GPCLR0: usize = 0x28;
pub const GPCLR1: usize = 0x2C;

/// GPIO Pin Level registers (GPLEV0 – GPLEV1).
/// Read returns the current level of each pin.
pub const GPLEV0: usize = 0x34;
pub const GPLEV1: usize = 0x38;

/// GPIO Pin Event Detect Status registers (GPEDS0 – GPEDS1).
pub const GPEDS0: usize = 0x40;
pub const GPEDS1: usize = 0x44;

/// GPIO Pull-up / Pull-down register (BCM2712 style, offset may vary
/// vs. legacy BCM2835). On the BCM2712 the pull control is via a single
/// register with 2-bit fields per pin.
pub const GPPUPPDN0: usize = 0xE4;
pub const GPPUPPDN1: usize = 0xE8;
pub const GPPUPPDN2: usize = 0xEC;
pub const GPPUPPDN3: usize = 0xF0;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// GPIO function selection.
///
/// Each GPIO pin can be assigned one of these functions. The encoding
/// matches the 3-bit FSEL field in the GPFSEL registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GpioFunction {
    /// Pin configured as a general-purpose input.
    Input   = 0b000,
    /// Pin configured as a general-purpose output.
    Output  = 0b001,
    /// Alternate function 0.
    AltFunc0 = 0b100,
    /// Alternate function 1.
    AltFunc1 = 0b101,
    /// Alternate function 2.
    AltFunc2 = 0b110,
    /// Alternate function 3.
    AltFunc3 = 0b111,
    /// Alternate function 4.
    AltFunc4 = 0b011,
    /// Alternate function 5.
    AltFunc5 = 0b010,
}

/// Internal pull resistor configuration for a GPIO pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GpioPull {
    /// No pull resistor (floating).
    None    = 0b00,
    /// Pull-up resistor enabled.
    PullUp  = 0b01,
    /// Pull-down resistor enabled.
    PullDown = 0b10,
}

// ---------------------------------------------------------------------------
// GpioPin
// ---------------------------------------------------------------------------

/// A handle to a single GPIO pin on the BCM2712.
///
/// # Safety
///
/// Constructing a `GpioPin` does not by itself configure the pin. The caller
/// is responsible for setting the pin function before reading or writing.
#[derive(Debug)]
pub struct GpioPin {
    /// GPIO pin number (0–57 on BCM2712).
    pin: u8,
    /// Base address of the GPIO register block.
    base: Address,
}

impl GpioPin {
    /// Creates a new handle for the given GPIO pin number.
    ///
    /// `base` must be the physical address of the GPIO register block
    /// (see [`crate::platform::Pi5Platform::gpio_base`]).
    #[inline]
    pub const fn new(pin: u8, base: Address) -> Self {
        Self { pin, base }
    }

    /// Returns the pin number.
    #[inline]
    pub const fn pin(&self) -> u8 {
        self.pin
    }

    // -----------------------------------------------------------------------
    // Function select
    // -----------------------------------------------------------------------

    /// Sets the function (input, output, alt) for this pin.
    pub fn set_function(&self, function: GpioFunction) {
        let reg_index = (self.pin as usize) / 10;
        let bit_offset = ((self.pin as usize) % 10) * 3;

        let reg_addr = self.base + GPFSEL0 + reg_index * 4;
        let mut val = mmio_read(reg_addr);

        // Clear the 3-bit FSEL field, then set the new value.
        val &= !(0b111 << bit_offset);
        val |= (function as u32) << bit_offset;

        mmio_write(reg_addr, val);
    }

    // -----------------------------------------------------------------------
    // Output
    // -----------------------------------------------------------------------

    /// Sets the pin high (1). The pin must be configured as an output first.
    #[inline]
    pub fn set_output(&self, high: bool) {
        let reg = if (self.pin as usize) < 32 {
            GPSET0
        } else {
            GPSET1
        };
        let bit = if (self.pin as usize) < 32 {
            1u32 << self.pin
        } else {
            1u32 << (self.pin - 32)
        };

        if high {
            mmio_write(self.base + reg, bit);
        } else {
            mmio_write(self.base + (reg - GPSET0 + GPCLR0), bit);
        }
    }

    /// Sets the pin high.
    #[inline]
    pub fn set_high(&self) {
        self.set_output(true);
    }

    /// Sets the pin low.
    #[inline]
    pub fn set_low(&self) {
        self.set_output(false);
    }

    // -----------------------------------------------------------------------
    // Input
    // -----------------------------------------------------------------------

    /// Reads the current logic level of the pin.
    ///
    /// Returns `true` if the pin is high, `false` if low.
    #[inline]
    pub fn read_input(&self) -> bool {
        let reg = if (self.pin as usize) < 32 {
            GPLEV0
        } else {
            GPLEV1
        };
        let bit = if (self.pin as usize) < 32 {
            1u32 << self.pin
        } else {
            1u32 << (self.pin - 32)
        };

        (mmio_read(self.base + reg) & bit) != 0
    }

    // -----------------------------------------------------------------------
    // Pull resistor
    // -----------------------------------------------------------------------

    /// Configures the internal pull resistor for this pin.
    ///
    /// On BCM2712 the pull configuration uses 2-bit fields in the
    /// GPPUPPDN registers (one register per 16 pins).
    pub fn set_pull(&self, pull: GpioPull) {
        let reg_index = (self.pin as usize) / 16;
        let bit_offset = ((self.pin as usize) % 16) * 2;

        let reg_addr = self.base + GPPUPPDN0 + reg_index * 4;
        let mut val = mmio_read(reg_addr);

        // Clear the 2-bit field, then set the new value.
        val &= !(0b11 << bit_offset);
        val |= (pull as u32) << bit_offset;

        mmio_write(reg_addr, val);
    }
}

// ---------------------------------------------------------------------------
// Tests (host-side, logic only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpio_function_encoding() {
        assert_eq!(GpioFunction::Input as u8, 0b000);
        assert_eq!(GpioFunction::Output as u8, 0b001);
        assert_eq!(GpioFunction::AltFunc5 as u8, 0b010);
        assert_eq!(GpioFunction::AltFunc4 as u8, 0b011);
        assert_eq!(GpioFunction::AltFunc0 as u8, 0b100);
    }

    #[test]
    fn gpio_pull_encoding() {
        assert_eq!(GpioPull::None as u8, 0b00);
        assert_eq!(GpioPull::PullUp as u8, 0b01);
        assert_eq!(GpioPull::PullDown as u8, 0b10);
    }

    #[test]
    fn pin_number_stored() {
        let pin = GpioPin::new(17, 0x1c00_0000);
        assert_eq!(pin.pin(), 17);
    }
}
