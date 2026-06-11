//! GPIO (General-Purpose Input/Output) driver for the Raspberry Pi 5.
//!
//! # RP1 I/O Chip
//!
//! The Pi 5 uses the BCM2712 SoC with an external **RP1 I/O co-processor**
//! connected via PCIe. GPIO pins are accessed through the RP1 at physical
//! address `0x1F00010000`, **not** through the legacy BCM2835-style registers
//! at `0x1C000000`.
//!
//! # RP1 Register Layout (from GPIO base)
//!
//! | Offset      | Register          | Description                     |
//! |-------------|-------------------|---------------------------------|
//! | `0x0000+n*4`| `GPIO_CTRL[n]`    | Per-pin function select         |
//! | `0x4000`    | `RIO_OUT`         | Output data (pins 0–31)         |
//! | `0x4004`    | `RIO_OUT`         | Output data (pins 32–53)        |
//! | `0x4008`    | `RIO_OE`          | Output enable (pins 0–31)       |
//! | `0x400C`    | `RIO_OE`          | Output enable (pins 32–53)      |
//! | `0x4010`    | `RIO_IN`          | Input data (pins 0–31)          |
//! | `0x4014`    | `RIO_IN`          | Input data (pins 32–53)         |
//! | `0x6000`    | `RIO_OUT_SET`     | Atomic output set (pins 0–31)   |
//! | `0x7000`    | `RIO_OUT_CLR`     | Atomic output clear (pins 0–31) |
//! | `0x6004`    | `RIO_OE_SET`      | Atomic OE set (pins 0–31)       |
//! | `0x7004`    | `RIO_OE_CLR`      | Atomic OE clear (pins 0–31)     |
//! | `0x8000+n*4`| `PAD_CTRL[n]`     | Per-pin pad control (pull, etc.)|
//!
//! # 40-Pin Header Mapping
//!
//! The Pi 5 retains the classic 40-pin header layout. [`HeaderPin`] provides
//! a mapping from header pin number (1–40) to the corresponding GPIO number,
//! power pin, or ground pin. Use [`pin_from_header`] or
//! [`GpioPin::from_header`] to obtain a `GpioPin` for a specific header
//! position.
//!
//! # PWM Support
//!
//! GPIO pins 12, 13, 18, and 19 support PWM output via the RP1's two
//! PWM channels. See [`GpioPwm`] for details.
//!
//! # Example
//!
//! ```no_run
//! use vuma_pi5::gpio::{GpioPin, GpioMode, GpioPull, gpio_set_mode, gpio_set_pull, gpio_write};
//! use vuma_pi5::platform::Pi5Platform;
//!
//! let platform = Pi5Platform::default();
//! let led = GpioPin::new(17, platform.rp1_gpio_base());
//! gpio_set_mode(&led, GpioMode::Output);
//! gpio_set_pull(&led, GpioPull::None);
//! gpio_write(&led, true);
//! ```

use crate::mmio::Address;

// ---------------------------------------------------------------------------
// Conditional MMIO: real hardware vs. mock for tests
// ---------------------------------------------------------------------------

#[cfg(not(test))]
use crate::mmio::{mmio_read, mmio_write};

/// Mock MMIO backend for host-side unit tests.
///
/// Uses a thread-local `HashMap` to simulate register state without
/// touching real hardware. Each test must call [`mock_mmio::reset`]
/// before use to start from a clean state.
#[cfg(test)]
mod mock_mmio {
    use crate::mmio::Address;
    use std::cell::RefCell;
    use std::collections::HashMap;

    std::thread_local! {
        static MOCK_REGS: RefCell<HashMap<Address, u32>> = RefCell::new(HashMap::new());
    }

    /// Clears all stored mock register values.
    pub fn reset() {
        MOCK_REGS.with(|regs| regs.borrow_mut().clear());
    }

    /// Reads a 32-bit value from the mock register file.
    ///
    /// Returns 0 for addresses that have never been written.
    pub fn read(addr: Address) -> u32 {
        MOCK_REGS.with(|regs| *regs.borrow().get(&addr).unwrap_or(&0))
    }

    /// Writes a 32-bit value to the mock register file.
    pub fn write(addr: Address, val: u32) {
        MOCK_REGS.with(|regs| regs.borrow_mut().insert(addr, val));
    }

    /// Mock replacement for [`crate::mmio::mmio_read`].
    #[inline]
    pub fn mmio_read(addr: Address) -> u32 {
        read(addr)
    }

    /// Mock replacement for [`crate::mmio::mmio_write`].
    #[inline]
    pub fn mmio_write(addr: Address, val: u32) {
        write(addr, val)
    }
}

#[cfg(test)]
use mock_mmio::{mmio_read, mmio_write};

// ===========================================================================
// RP1 GPIO register offset constants (relative to GPIO base)
// ===========================================================================

// ---------------------------------------------------------------------------
// GPIO_CTRL[n] — per-pin control register
// ---------------------------------------------------------------------------

/// Offset of the first `GPIO_CTRL` register from GPIO base.
/// Each pin has its own 4-byte register at `base + GPIO_CTRL_OFFSET + pin*4`.
pub const GPIO_CTRL_OFFSET: Address = 0x0000;

/// FUNCSEL field: bits [4:0] of GPIO_CTRL[n].
pub const FSEL_MASK: u32 = 0x1F;
/// Bit position of the FUNCSEL field within the GPIO_CTRL register.
pub const FSEL_SHIFT: u32 = 0;

/// OUTOVER field: bits [13:12] of GPIO_CTRL[n].
pub const OUTOVER_SHIFT: u32 = 12;
/// OEOVER field: bits [15:14] of GPIO_CTRL[n].
pub const OEOVER_SHIFT: u32 = 14;

// ---------------------------------------------------------------------------
// RP1 FUNCSEL encoding
// ---------------------------------------------------------------------------

/// FUNCSEL value for GPIO mode. Direction is then set via RIO_OE.
pub const FSEL_GPIO: u32 = 4;
/// FUNCSEL value for ALT0.
pub const FSEL_ALT0: u32 = 0;
/// FUNCSEL value for ALT1.
pub const FSEL_ALT1: u32 = 1;
/// FUNCSEL value for ALT2.
pub const FSEL_ALT2: u32 = 2;
/// FUNCSEL value for ALT3.
pub const FSEL_ALT3: u32 = 3;
/// FUNCSEL value for ALT4.
pub const FSEL_ALT4: u32 = 5;
/// FUNCSEL value for ALT5.
pub const FSEL_ALT5: u32 = 6;

// ---------------------------------------------------------------------------
// RIO (Register I/O) section
// ---------------------------------------------------------------------------

/// RIO_OUT: read current output level, per bank.
pub const RIO_OUT_OFFSET: Address = 0x4000;
/// RIO_OE: read current output-enable, per bank.
pub const RIO_OE_OFFSET: Address = 0x4004;
/// RIO_IN: read current input level, per bank.
pub const RIO_IN_OFFSET: Address = 0x4008;

/// RIO_OUT_SET: write-1-to-atomically-set output (per bank).
pub const RIO_OUT_SET_OFFSET: Address = 0x6000;
/// RIO_OUT_CLR: write-1-to-atomically-clear output (per bank).
pub const RIO_OUT_CLR_OFFSET: Address = 0x7000;
/// RIO_OE_SET: write-1-to-atomically-enable output (per bank).
pub const RIO_OE_SET_OFFSET: Address = 0x6004;
/// RIO_OE_CLR: write-1-to-atomically-disable output (per bank).
pub const RIO_OE_CLR_OFFSET: Address = 0x7004;

/// Bank stride: each RIO register has a bank-1 copy for pins 32–53,
/// located 4 bytes after the bank-0 register.
pub const RIO_BANK_STRIDE: Address = 4;

// ---------------------------------------------------------------------------
// PAD control section
// ---------------------------------------------------------------------------

/// Base offset of per-pin PAD control registers.
/// `PAD_CTRL[n]` is at `base + PAD_CTRL_OFFSET + n*4`.
pub const PAD_CTRL_OFFSET: Address = 0x8000;

/// Pull resistor field: bits [23:22] of PAD_CTRL[n].
pub const PAD_PULL_SHIFT: u32 = 22;
/// Bitmask for the pull resistor field in PAD_CTRL[n].
pub const PAD_PULL_MASK: u32 = 0x3 << PAD_PULL_SHIFT;

/// Drive strength field: bits [1:0] of PAD_CTRL[n].
///
/// On the RP1, the drive strength is configurable per-pin:
/// - 0b00 = 2 mA
/// - 0b01 = 4 mA
/// - 0b10 = 8 mA
/// - 0b11 = 12 mA
pub const PAD_DRIVE_SHIFT: u32 = 0;
/// Bitmask for the drive strength field in PAD_CTRL[n].
pub const PAD_DRIVE_MASK: u32 = 0x3 << PAD_DRIVE_SHIFT;

/// Input enable bit: bit [6] of PAD_CTRL[n].
///
/// When set, the pin's input buffer is enabled. When clear, the
/// input is disabled to save power.
pub const PAD_IN_ENABLE_BIT: u32 = 1 << 6;

/// Schmitt trigger enable bit: bit [9] of PAD_CTRL[n].
///
/// When set, the pin's input uses a Schmitt trigger for noise
/// immunity on slowly-changing signals.
pub const PAD_SCHMITT_BIT: u32 = 1 << 9;

/// Slew rate control bit: bit [10] of PAD_CTRL[n].
///
/// When set, the output uses fast slew rate. When clear, slow slew.
pub const PAD_SLEW_FAST_BIT: u32 = 1 << 10;

// ---------------------------------------------------------------------------
// PWM register offsets (relative to PWM channel base)
// ---------------------------------------------------------------------------

/// PWM control register offset.
pub const PWM_CTRL_OFFSET: Address = 0x00;
/// PWM range (period) register offset.
pub const PWM_RNG_OFFSET: Address = 0x04;
/// PWM data (duty cycle) register offset.
pub const PWM_DAT_OFFSET: Address = 0x08;

/// PWM control register: enable bit.
pub const PWM_CTRL_ENABLE: u32 = 1 << 0;
/// PWM control register: MSEN (MSB-first) bit.
pub const PWM_CTRL_MSEN: u32 = 1 << 4;

/// GPIO pins that support PWM on the RP1.
///
/// - GPIO12 → PWM0 via AltFunc0
/// - GPIO13 → PWM1 via AltFunc0
/// - GPIO18 → PWM0 via AltFunc5
/// - GPIO19 → PWM1 via AltFunc5
pub const PWM_GPIO_PINS: [u8; 4] = [12, 13, 18, 19];

// ===========================================================================
// Enums
// ===========================================================================

/// GPIO pin mode / function selection.
///
/// On the RP1, both `Input` and `Output` use FUNCSEL = 4 (GPIO mode).
/// The actual direction is then controlled by the RIO output-enable register.
/// Alternate functions map directly to RP1 FUNCSEL values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpioMode {
    /// Pin configured as general-purpose input (FUNCSEL=4, OE=0).
    Input,
    /// Pin configured as general-purpose output (FUNCSEL=4, OE=1).
    Output,
    /// Alternate function 0 (FUNCSEL=0).
    AltFunc0,
    /// Alternate function 1 (FUNCSEL=1).
    AltFunc1,
    /// Alternate function 2 (FUNCSEL=2).
    AltFunc2,
    /// Alternate function 3 (FUNCSEL=3).
    AltFunc3,
    /// Alternate function 4 (FUNCSEL=5).
    AltFunc4,
    /// Alternate function 5 (FUNCSEL=6).
    AltFunc5,
}

impl GpioMode {
    /// Returns the RP1 FUNCSEL register value for this mode.
    #[inline]
    pub const fn funcsels(&self) -> u32 {
        match self {
            GpioMode::Input | GpioMode::Output => FSEL_GPIO,
            GpioMode::AltFunc0 => FSEL_ALT0,
            GpioMode::AltFunc1 => FSEL_ALT1,
            GpioMode::AltFunc2 => FSEL_ALT2,
            GpioMode::AltFunc3 => FSEL_ALT3,
            GpioMode::AltFunc4 => FSEL_ALT4,
            GpioMode::AltFunc5 => FSEL_ALT5,
        }
    }

    /// Returns `true` if this mode requires the RIO output-enable to be set.
    #[inline]
    pub const fn is_output(&self) -> bool {
        matches!(self, GpioMode::Output)
    }
}

/// Internal pull resistor configuration for a GPIO pin.
///
/// On the RP1, the pull setting is encoded in bits [23:22] of the
/// per-pin `PAD_CTRL` register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GpioPull {
    /// No pull resistor (floating).
    None = 0b00,
    /// Pull-down resistor enabled.
    PullDown = 0b01,
    /// Pull-up resistor enabled.
    PullUp = 0b10,
}

/// Output drive strength configuration for a GPIO pin.
///
/// On the RP1, the drive strength is encoded in bits [1:0] of the
/// per-pin `PAD_CTRL` register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DriveStrength {
    /// 2 mA drive strength.
    Ma2 = 0b00,
    /// 4 mA drive strength.
    Ma4 = 0b01,
    /// 8 mA drive strength.
    Ma8 = 0b10,
    /// 12 mA drive strength.
    Ma12 = 0b11,
}

// ===========================================================================
// 40-Pin Header Mapping
// ===========================================================================

/// Classification of a Raspberry Pi 5 40-pin header pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderPinKind {
    /// 3.3 V power.
    Power3V3,
    /// 5 V power.
    Power5V,
    /// Ground.
    Ground,
    /// A GPIO pin with the given GPIO number.
    Gpio(u8),
    /// Reserved / not connected.
    Reserved,
}

/// Describes a single pin on the 40-pin header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderPin {
    /// Physical pin number on the 40-pin header (1–40).
    pub pin: u8,
    /// What kind of signal this pin carries.
    pub kind: HeaderPinKind,
    /// Default function on the Pi 5 (human-readable label).
    pub default_func: &'static str,
}

/// Complete 40-pin header mapping for the Raspberry Pi 5.
///
/// Pin assignments follow the standard Pi 40-pin layout:
///
/// ```text
///  3V3  (1)  (2)  5V
///  GPIO2  (3)  (4)  5V
///  GPIO3  (5)  (6)  GND
///  GPIO4  (7)  (8)  GPIO14
///    GND  (9) (10)  GPIO15
/// GPIO17 (11) (12)  GPIO18
/// GPIO27 (13) (14)  GND
/// GPIO22 (15) (16)  GPIO23
///   3V3 (17) (18)  GPIO24
/// GPIO10 (19) (20)  GND
///  GPIO9 (21) (22)  GPIO25
/// GPIO11 (23) (24)  GPIO8
///   GND (25) (26)  GPIO7
///  GPIO0 (27) (28)  GPIO1
///  GPIO5 (29) (30)  GND
///  GPIO6 (31) (32)  GPIO12
/// GPIO13 (33) (34)  GND
/// GPIO19 (35) (36)  GPIO16
/// GPIO26 (37) (38)  GPIO20
///   GND (39) (40)  GPIO21
/// ```
pub const HEADER_PINS: [HeaderPin; 40] = [
    HeaderPin {
        pin: 1,
        kind: HeaderPinKind::Power3V3,
        default_func: "3V3",
    },
    HeaderPin {
        pin: 2,
        kind: HeaderPinKind::Power5V,
        default_func: "5V",
    },
    HeaderPin {
        pin: 3,
        kind: HeaderPinKind::Gpio(2),
        default_func: "SDA1",
    },
    HeaderPin {
        pin: 4,
        kind: HeaderPinKind::Power5V,
        default_func: "5V",
    },
    HeaderPin {
        pin: 5,
        kind: HeaderPinKind::Gpio(3),
        default_func: "SCL1",
    },
    HeaderPin {
        pin: 6,
        kind: HeaderPinKind::Ground,
        default_func: "GND",
    },
    HeaderPin {
        pin: 7,
        kind: HeaderPinKind::Gpio(4),
        default_func: "GPCLK0",
    },
    HeaderPin {
        pin: 8,
        kind: HeaderPinKind::Gpio(14),
        default_func: "TXD0",
    },
    HeaderPin {
        pin: 9,
        kind: HeaderPinKind::Ground,
        default_func: "GND",
    },
    HeaderPin {
        pin: 10,
        kind: HeaderPinKind::Gpio(15),
        default_func: "RXD0",
    },
    HeaderPin {
        pin: 11,
        kind: HeaderPinKind::Gpio(17),
        default_func: "GPIO17",
    },
    HeaderPin {
        pin: 12,
        kind: HeaderPinKind::Gpio(18),
        default_func: "PWM0",
    },
    HeaderPin {
        pin: 13,
        kind: HeaderPinKind::Gpio(27),
        default_func: "GPIO27",
    },
    HeaderPin {
        pin: 14,
        kind: HeaderPinKind::Ground,
        default_func: "GND",
    },
    HeaderPin {
        pin: 15,
        kind: HeaderPinKind::Gpio(22),
        default_func: "GPIO22",
    },
    HeaderPin {
        pin: 16,
        kind: HeaderPinKind::Gpio(23),
        default_func: "GPIO23",
    },
    HeaderPin {
        pin: 17,
        kind: HeaderPinKind::Power3V3,
        default_func: "3V3",
    },
    HeaderPin {
        pin: 18,
        kind: HeaderPinKind::Gpio(24),
        default_func: "GPIO24",
    },
    HeaderPin {
        pin: 19,
        kind: HeaderPinKind::Gpio(10),
        default_func: "MOSI0",
    },
    HeaderPin {
        pin: 20,
        kind: HeaderPinKind::Ground,
        default_func: "GND",
    },
    HeaderPin {
        pin: 21,
        kind: HeaderPinKind::Gpio(9),
        default_func: "MISO0",
    },
    HeaderPin {
        pin: 22,
        kind: HeaderPinKind::Gpio(25),
        default_func: "GPIO25",
    },
    HeaderPin {
        pin: 23,
        kind: HeaderPinKind::Gpio(11),
        default_func: "SCLK0",
    },
    HeaderPin {
        pin: 24,
        kind: HeaderPinKind::Gpio(8),
        default_func: "CE0",
    },
    HeaderPin {
        pin: 25,
        kind: HeaderPinKind::Ground,
        default_func: "GND",
    },
    HeaderPin {
        pin: 26,
        kind: HeaderPinKind::Gpio(7),
        default_func: "CE1",
    },
    HeaderPin {
        pin: 27,
        kind: HeaderPinKind::Gpio(0),
        default_func: "SDA0",
    },
    HeaderPin {
        pin: 28,
        kind: HeaderPinKind::Gpio(1),
        default_func: "SCL0",
    },
    HeaderPin {
        pin: 29,
        kind: HeaderPinKind::Gpio(5),
        default_func: "GPCLK1",
    },
    HeaderPin {
        pin: 30,
        kind: HeaderPinKind::Ground,
        default_func: "GND",
    },
    HeaderPin {
        pin: 31,
        kind: HeaderPinKind::Gpio(6),
        default_func: "GPCLK2",
    },
    HeaderPin {
        pin: 32,
        kind: HeaderPinKind::Gpio(12),
        default_func: "PWM0",
    },
    HeaderPin {
        pin: 33,
        kind: HeaderPinKind::Gpio(13),
        default_func: "PWM1",
    },
    HeaderPin {
        pin: 34,
        kind: HeaderPinKind::Ground,
        default_func: "GND",
    },
    HeaderPin {
        pin: 35,
        kind: HeaderPinKind::Gpio(19),
        default_func: "PWM1",
    },
    HeaderPin {
        pin: 36,
        kind: HeaderPinKind::Gpio(16),
        default_func: "GPIO16",
    },
    HeaderPin {
        pin: 37,
        kind: HeaderPinKind::Gpio(26),
        default_func: "GPIO26",
    },
    HeaderPin {
        pin: 38,
        kind: HeaderPinKind::Gpio(20),
        default_func: "GPIO20",
    },
    HeaderPin {
        pin: 39,
        kind: HeaderPinKind::Ground,
        default_func: "GND",
    },
    HeaderPin {
        pin: 40,
        kind: HeaderPinKind::Gpio(21),
        default_func: "GPIO21",
    },
];

/// Returns the [`HeaderPin`] description for the given physical pin number
/// (1–40), or `None` if the pin number is out of range.
#[inline]
pub fn header_pin(pin: u8) -> Option<&'static HeaderPin> {
    if (1..=40).contains(&pin) {
        Some(&HEADER_PINS[(pin - 1) as usize])
    } else {
        None
    }
}

/// Returns the GPIO number for a given 40-pin header pin, if that pin
/// carries a GPIO signal.
///
/// Returns `None` for power, ground, or reserved pins.
#[inline]
pub fn gpio_from_header(pin: u8) -> Option<u8> {
    header_pin(pin).and_then(|hp| match hp.kind {
        HeaderPinKind::Gpio(n) => Some(n),
        _ => None,
    })
}

/// Returns the default alternate function for a PWM-capable GPIO pin.
///
/// | GPIO | PWM Channel | Alt Function |
/// |------|-------------|--------------|
/// | 12   | PWM0        | AltFunc0     |
/// | 13   | PWM1        | AltFunc0     |
/// | 18   | PWM0        | AltFunc5     |
/// | 19   | PWM1        | AltFunc5     |
///
/// Returns `None` for GPIO pins that don't support PWM.
pub fn pwm_alt_func_for_gpio(gpio: u8) -> Option<GpioMode> {
    match gpio {
        12 => Some(GpioMode::AltFunc0), // PWM0
        13 => Some(GpioMode::AltFunc0), // PWM1
        18 => Some(GpioMode::AltFunc5), // PWM0
        19 => Some(GpioMode::AltFunc5), // PWM1
        _ => None,
    }
}

// ===========================================================================
// GpioPin
// ===========================================================================

/// A handle to a single GPIO pin on the Raspberry Pi 5 via the RP1.
///
/// # Safety
///
/// Constructing a `GpioPin` does not by itself configure the pin. The caller
/// is responsible for setting the pin mode before reading or writing.
///
/// # Example
///
/// ```no_run
/// use vuma_pi5::gpio::{GpioPin, GpioMode, GpioPull};
/// use vuma_pi5::platform::Pi5Platform;
///
/// let p = Pi5Platform::default();
/// let pin = GpioPin::new(17, p.rp1_gpio_base());
/// pin.set_mode(GpioMode::Output);
/// pin.set_pull(GpioPull::None);
/// pin.write(true);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct GpioPin {
    /// GPIO pin number (0–53 on RP1).
    pin: u8,
    /// Base address of the RP1 GPIO register block.
    base: Address,
}

impl GpioPin {
    /// Creates a new handle for the given GPIO pin number.
    ///
    /// `base` must be the physical address of the RP1 GPIO register block
    /// (see [`crate::platform::Pi5Platform::rp1_gpio_base`]).
    #[inline]
    pub const fn new(pin: u8, base: Address) -> Self {
        Self { pin, base }
    }

    /// Creates a `GpioPin` from a 40-pin header pin number.
    ///
    /// Returns `None` if the header pin is a power or ground pin, or if the
    /// pin number is out of range (1–40).
    #[inline]
    pub fn from_header(header_pin: u8, base: Address) -> Option<Self> {
        gpio_from_header(header_pin).map(|gpio| Self::new(gpio, base))
    }

    /// Returns the pin number.
    #[inline]
    pub const fn pin(&self) -> u8 {
        self.pin
    }

    /// Returns the base address of the RP1 GPIO register block.
    #[inline]
    pub const fn base(&self) -> Address {
        self.base
    }

    // -----------------------------------------------------------------------
    // Internal register-address helpers
    // -----------------------------------------------------------------------

    /// Address of `GPIO_CTRL[self.pin]`.
    #[inline]
    fn ctrl_addr(&self) -> Address {
        self.base + GPIO_CTRL_OFFSET + (self.pin as Address) * 4
    }

    /// RIO bank offset: 0 for pins 0–31, 4 for pins 32–53.
    #[inline]
    fn rio_bank(&self) -> Address {
        (self.pin as Address / 32) * RIO_BANK_STRIDE
    }

    /// Bit mask for this pin within its RIO bank register.
    #[inline]
    fn rio_mask(&self) -> u32 {
        1u32 << (self.pin % 32)
    }

    /// Address of `PAD_CTRL[self.pin]`.
    #[inline]
    fn pad_addr(&self) -> Address {
        self.base + PAD_CTRL_OFFSET + (self.pin as Address) * 4
    }

    // -----------------------------------------------------------------------
    // Mode / function select
    // -----------------------------------------------------------------------

    /// Sets the mode (input, output, alternate function) for this pin.
    ///
    /// For [`GpioMode::Input`], the RIO output-enable is cleared.
    /// For [`GpioMode::Output`], the RIO output-enable is set.
    /// For alternate functions, the FUNCSEL is programmed and OE is cleared.
    pub fn set_mode(&self, mode: GpioMode) {
        // Program FUNCSEL in GPIO_CTRL[pin]
        let ctrl_addr = self.ctrl_addr();
        let mut ctrl = mmio_read(ctrl_addr);
        ctrl &= !(FSEL_MASK << FSEL_SHIFT);
        ctrl |= mode.funcsels() << FSEL_SHIFT;
        mmio_write(ctrl_addr, ctrl);

        // Set or clear output-enable via RIO atomic SET/CLR
        if mode.is_output() {
            mmio_write(
                self.base + RIO_OE_SET_OFFSET + self.rio_bank(),
                self.rio_mask(),
            );
        } else {
            mmio_write(
                self.base + RIO_OE_CLR_OFFSET + self.rio_bank(),
                self.rio_mask(),
            );
        }
    }

    /// Reads the current mode of this pin from the GPIO_CTRL register.
    ///
    /// Returns the FUNCSEL value. Note: Input and Output share FUNCSEL=4;
    /// use [`read_oe`] to determine the direction.
    ///
    /// [`read_oe`]: GpioPin::read_oe
    pub fn read_mode(&self) -> u32 {
        let ctrl = mmio_read(self.ctrl_addr());
        (ctrl >> FSEL_SHIFT) & FSEL_MASK
    }

    // -----------------------------------------------------------------------
    // Pull resistor
    // -----------------------------------------------------------------------

    /// Configures the internal pull resistor for this pin.
    ///
    /// On the RP1, pull settings are in bits [23:22] of the per-pin
    /// `PAD_CTRL` register.
    pub fn set_pull(&self, pull: GpioPull) {
        let pad_addr = self.pad_addr();
        let mut pad = mmio_read(pad_addr);
        pad &= !PAD_PULL_MASK;
        pad |= (pull as u32) << PAD_PULL_SHIFT;
        mmio_write(pad_addr, pad);
    }

    /// Reads the current pull resistor configuration for this pin.
    pub fn read_pull(&self) -> GpioPull {
        let pad = mmio_read(self.pad_addr());
        let raw = (pad & PAD_PULL_MASK) >> PAD_PULL_SHIFT;
        match raw {
            0b00 => GpioPull::None,
            0b01 => GpioPull::PullDown,
            0b10 => GpioPull::PullUp,
            _ => GpioPull::None, // 0b11 is reserved; treat as None
        }
    }

    // -----------------------------------------------------------------------
    // BCM2712 RP1 advanced pad control
    // -----------------------------------------------------------------------

    /// Configures the output drive strength for this pin.
    ///
    /// On the RP1, drive strength is encoded in bits [1:0] of the
    /// per-pin `PAD_CTRL` register. Higher drive strength allows the
    /// pin to source/sink more current.
    pub fn set_drive_strength(&self, strength: DriveStrength) {
        let pad_addr = self.pad_addr();
        let mut pad = mmio_read(pad_addr);
        pad &= !PAD_DRIVE_MASK;
        pad |= (strength as u32) << PAD_DRIVE_SHIFT;
        mmio_write(pad_addr, pad);
    }

    /// Reads the current drive strength configuration for this pin.
    pub fn read_drive_strength(&self) -> DriveStrength {
        let pad = mmio_read(self.pad_addr());
        let raw = (pad & PAD_DRIVE_MASK) >> PAD_DRIVE_SHIFT;
        match raw {
            0b00 => DriveStrength::Ma2,
            0b01 => DriveStrength::Ma4,
            0b10 => DriveStrength::Ma8,
            0b11 => DriveStrength::Ma12,
            _ => DriveStrength::Ma2, // unreachable
        }
    }

    /// Enables or disables the input buffer for this pin.
    ///
    /// Disabling the input buffer saves power when the pin is used
    /// solely as an output.
    pub fn set_input_enable(&self, enable: bool) {
        let pad_addr = self.pad_addr();
        let mut pad = mmio_read(pad_addr);
        if enable {
            pad |= PAD_IN_ENABLE_BIT;
        } else {
            pad &= !PAD_IN_ENABLE_BIT;
        }
        mmio_write(pad_addr, pad);
    }

    /// Returns `true` if the input buffer is enabled for this pin.
    pub fn read_input_enable(&self) -> bool {
        (mmio_read(self.pad_addr()) & PAD_IN_ENABLE_BIT) != 0
    }

    /// Enables or disables the Schmitt trigger for this pin.
    ///
    /// The Schmitt trigger provides hysteresis on the input, improving
    /// noise immunity for slowly-changing signals.
    pub fn set_schmitt(&self, enable: bool) {
        let pad_addr = self.pad_addr();
        let mut pad = mmio_read(pad_addr);
        if enable {
            pad |= PAD_SCHMITT_BIT;
        } else {
            pad &= !PAD_SCHMITT_BIT;
        }
        mmio_write(pad_addr, pad);
    }

    /// Returns `true` if the Schmitt trigger is enabled for this pin.
    pub fn read_schmitt(&self) -> bool {
        (mmio_read(self.pad_addr()) & PAD_SCHMITT_BIT) != 0
    }

    /// Enables or disables fast slew rate for this pin.
    ///
    /// When enabled, the output transitions quickly. When disabled,
    /// the output transitions slowly, reducing EMI.
    pub fn set_slew_fast(&self, enable: bool) {
        let pad_addr = self.pad_addr();
        let mut pad = mmio_read(pad_addr);
        if enable {
            pad |= PAD_SLEW_FAST_BIT;
        } else {
            pad &= !PAD_SLEW_FAST_BIT;
        }
        mmio_write(pad_addr, pad);
    }

    /// Returns `true` if fast slew rate is enabled for this pin.
    pub fn read_slew_fast(&self) -> bool {
        (mmio_read(self.pad_addr()) & PAD_SLEW_FAST_BIT) != 0
    }

    // -----------------------------------------------------------------------
    // Output
    // -----------------------------------------------------------------------

    /// Sets the pin output level. The pin must be configured as output first.
    ///
    /// Uses the RP1's atomic RIO_OUT_SET / RIO_OUT_CLR registers so no
    /// read-modify-write cycle is needed.
    pub fn write(&self, high: bool) {
        if high {
            mmio_write(
                self.base + RIO_OUT_SET_OFFSET + self.rio_bank(),
                self.rio_mask(),
            );
        } else {
            mmio_write(
                self.base + RIO_OUT_CLR_OFFSET + self.rio_bank(),
                self.rio_mask(),
            );
        }
    }

    /// Sets the pin high. Convenience alias for `write(true)`.
    #[inline]
    pub fn set_high(&self) {
        self.write(true);
    }

    /// Sets the pin low. Convenience alias for `write(false)`.
    #[inline]
    pub fn set_low(&self) {
        self.write(false);
    }

    /// Toggles the pin output level.
    ///
    /// Reads the current RIO_OUT state and atomically sets or clears the
    /// output. The pin must be configured as output first.
    pub fn toggle(&self) {
        let out_addr = self.base + RIO_OUT_OFFSET + self.rio_bank();
        let current = mmio_read(out_addr);
        if (current & self.rio_mask()) != 0 {
            self.write(false);
        } else {
            self.write(true);
        }
    }

    // -----------------------------------------------------------------------
    // Input
    // -----------------------------------------------------------------------

    /// Reads the current logic level of the pin.
    ///
    /// Returns `true` if the pin is high, `false` if low.
    pub fn read(&self) -> bool {
        let reg_addr = self.base + RIO_IN_OFFSET + self.rio_bank();
        (mmio_read(reg_addr) & self.rio_mask()) != 0
    }

    /// Reads the current output-enable state for this pin.
    ///
    /// Returns `true` if the pin's output-enable is set.
    pub fn read_oe(&self) -> bool {
        let oe_addr = self.base + RIO_OE_OFFSET + self.rio_bank();
        (mmio_read(oe_addr) & self.rio_mask()) != 0
    }

    /// Reads the current output level (reflected in RIO_OUT) for this pin.
    ///
    /// This is the level being driven on the pin when configured as output.
    /// Returns `true` if the output is high.
    pub fn read_out(&self) -> bool {
        let out_addr = self.base + RIO_OUT_OFFSET + self.rio_bank();
        (mmio_read(out_addr) & self.rio_mask()) != 0
    }
}

// ===========================================================================
// Free functions (convenience wrappers)
// ===========================================================================

/// Sets the mode (input, output, alt function) for a GPIO pin.
///
/// Convenience wrapper around [`GpioPin::set_mode`].
#[inline]
pub fn gpio_set_mode(pin: &GpioPin, mode: GpioMode) {
    pin.set_mode(mode);
}

/// Configures the internal pull resistor for a GPIO pin.
///
/// Convenience wrapper around [`GpioPin::set_pull`].
#[inline]
pub fn gpio_set_pull(pin: &GpioPin, pull: GpioPull) {
    pin.set_pull(pull);
}

/// Sets the output level of a GPIO pin.
///
/// Convenience wrapper around [`GpioPin::write`].
#[inline]
pub fn gpio_write(pin: &GpioPin, value: bool) {
    pin.write(value);
}

/// Reads the current input level of a GPIO pin.
///
/// Convenience wrapper around [`GpioPin::read`].
#[inline]
pub fn gpio_read(pin: &GpioPin) -> bool {
    pin.read()
}

/// Configures the output drive strength for a GPIO pin.
///
/// Convenience wrapper around [`GpioPin::set_drive_strength`].
#[inline]
pub fn gpio_set_drive_strength(pin: &GpioPin, strength: DriveStrength) {
    pin.set_drive_strength(strength);
}

/// Enables or disables the input buffer for a GPIO pin.
///
/// Convenience wrapper around [`GpioPin::set_input_enable`].
#[inline]
pub fn gpio_set_input_enable(pin: &GpioPin, enable: bool) {
    pin.set_input_enable(enable);
}

/// Enables or disables the Schmitt trigger for a GPIO pin.
///
/// Convenience wrapper around [`GpioPin::set_schmitt`].
#[inline]
pub fn gpio_set_schmitt(pin: &GpioPin, enable: bool) {
    pin.set_schmitt(enable);
}

/// Enables or disables fast slew rate for a GPIO pin.
///
/// Convenience wrapper around [`GpioPin::set_slew_fast`].
#[inline]
pub fn gpio_set_slew_fast(pin: &GpioPin, enable: bool) {
    pin.set_slew_fast(enable);
}

// ===========================================================================
// PWM
// ===========================================================================

/// PWM channel selection on the RP1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PwmChannel {
    /// PWM channel 0 — available on GPIO12 (AltFunc0) and GPIO18 (AltFunc5).
    Pwm0,
    /// PWM channel 1 — available on GPIO13 (AltFunc0) and GPIO19 (AltFunc5).
    Pwm1,
}

impl PwmChannel {
    /// Returns the GPIO pin numbers that support this PWM channel.
    pub const fn gpio_pins(&self) -> [u8; 2] {
        match self {
            PwmChannel::Pwm0 => [12, 18],
            PwmChannel::Pwm1 => [13, 19],
        }
    }
}

/// PWM output controller for the RP1.
///
/// The RP1 provides two independent PWM channels. Before enabling PWM output,
/// the corresponding GPIO pin must be configured with the appropriate
/// alternate function:
///
/// | GPIO  | Channel | Alt Function |
/// |-------|---------|--------------|
/// | 12    | PWM0    | AltFunc0     |
/// | 13    | PWM1    | AltFunc0     |
/// | 18    | PWM0    | AltFunc5     |
/// | 19    | PWM1    | AltFunc5     |
///
/// # Example
///
/// ```no_run
/// use vuma_pi5::gpio::{GpioPin, GpioMode, GpioPwm, PwmChannel};
/// use vuma_pi5::platform::Pi5Platform;
///
/// let p = Pi5Platform::default();
///
/// // Configure GPIO12 for PWM0
/// let pin = GpioPin::new(12, p.rp1_gpio_base());
/// pin.set_mode(GpioMode::AltFunc0);
///
/// // Set up and enable PWM0
/// let pwm = GpioPwm::new(PwmChannel::Pwm0, p.rp1_pwm0_base());
/// pwm.set_range(1024);
/// pwm.set_duty_cycle(512);
/// pwm.enable();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct GpioPwm {
    /// PWM channel identifier.
    channel: PwmChannel,
    /// Base address of this PWM channel's register block.
    base: Address,
}

impl GpioPwm {
    /// Creates a new PWM controller handle for the given channel.
    ///
    /// `base` must be the physical address of the PWM channel's register block
    /// (see [`crate::platform::Pi5Platform::rp1_pwm0_base`] or
    /// [`crate::platform::Pi5Platform::rp1_pwm1_base`]).
    #[inline]
    pub const fn new(channel: PwmChannel, base: Address) -> Self {
        Self { channel, base }
    }

    /// Creates a `GpioPwm` from a PWM-capable GPIO pin.
    ///
    /// Returns `None` if the GPIO pin does not support PWM.
    pub fn from_gpio(gpio: u8, base: Address) -> Option<Self> {
        match gpio {
            12 | 18 => Some(Self::new(PwmChannel::Pwm0, base)),
            13 | 19 => Some(Self::new(PwmChannel::Pwm1, base)),
            _ => None,
        }
    }

    /// Returns the PWM channel.
    #[inline]
    pub const fn channel(&self) -> PwmChannel {
        self.channel
    }

    /// Returns the base address of the PWM register block.
    #[inline]
    pub const fn base(&self) -> Address {
        self.base
    }

    /// Enables the PWM output.
    pub fn enable(&self) {
        let ctrl = mmio_read(self.base + PWM_CTRL_OFFSET);
        mmio_write(self.base + PWM_CTRL_OFFSET, ctrl | PWM_CTRL_ENABLE);
    }

    /// Disables the PWM output.
    pub fn disable(&self) {
        let ctrl = mmio_read(self.base + PWM_CTRL_OFFSET);
        mmio_write(self.base + PWM_CTRL_OFFSET, ctrl & !PWM_CTRL_ENABLE);
    }

    /// Returns `true` if the PWM channel is currently enabled.
    pub fn is_enabled(&self) -> bool {
        (mmio_read(self.base + PWM_CTRL_OFFSET) & PWM_CTRL_ENABLE) != 0
    }

    /// Sets the range (period) value for the PWM channel.
    ///
    /// The PWM output frequency is `clock_frequency / range`.
    pub fn set_range(&self, range: u32) {
        mmio_write(self.base + PWM_RNG_OFFSET, range);
    }

    /// Sets the duty-cycle data value for the PWM channel.
    ///
    /// The duty cycle ratio is `duty / range`.
    pub fn set_duty_cycle(&self, duty: u32) {
        mmio_write(self.base + PWM_DAT_OFFSET, duty);
    }

    /// Returns the current range (period) value.
    pub fn range(&self) -> u32 {
        mmio_read(self.base + PWM_RNG_OFFSET)
    }

    /// Returns the current duty-cycle value.
    pub fn duty_cycle(&self) -> u32 {
        mmio_read(self.base + PWM_DAT_OFFSET)
    }

    /// Configures the PWM channel with MSEN (Mark-Space ERable) mode for
    /// more precise duty-cycle control at low ratios.
    pub fn enable_msen(&self) {
        let ctrl = mmio_read(self.base + PWM_CTRL_OFFSET);
        mmio_write(self.base + PWM_CTRL_OFFSET, ctrl | PWM_CTRL_MSEN);
    }

    /// Disables MSEN mode.
    pub fn disable_msen(&self) {
        let ctrl = mmio_read(self.base + PWM_CTRL_OFFSET);
        mmio_write(self.base + PWM_CTRL_OFFSET, ctrl & !PWM_CTRL_MSEN);
    }

    /// Returns `true` if MSEN mode is enabled.
    pub fn is_msen(&self) -> bool {
        (mmio_read(self.base + PWM_CTRL_OFFSET) & PWM_CTRL_MSEN) != 0
    }
}

// ===========================================================================
// Tests (host-side, using mock MMIO)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Pi5Platform, RP1_GPIO_BASE, RP1_PWM0_BASE, RP1_PWM1_BASE};

    /// Arbitrary non-zero base address used for mock GPIO registers.
    const MOCK_BASE: Address = 0x1000_0000;
    /// Arbitrary base address for mock PWM0 registers.
    const MOCK_PWM0_BASE: Address = 0x2000_0000;
    /// Arbitrary base address for mock PWM1 registers.
    const MOCK_PWM1_BASE: Address = 0x2001_0000;

    /// Resets the mock MMIO store before each test.
    fn reset_mock() {
        mock_mmio::reset();
    }

    // ---- Test 1: Set pin to Input mode -----------------------------------

    #[test]
    fn test_set_mode_input() {
        reset_mock();
        let pin = GpioPin::new(17, MOCK_BASE);
        gpio_set_mode(&pin, GpioMode::Input);

        // GPIO_CTRL[17] should have FUNCSEL = 4 (GPIO)
        let ctrl_addr = MOCK_BASE + GPIO_CTRL_OFFSET + 17 * 4;
        let ctrl = mock_mmio::read(ctrl_addr);
        assert_eq!(
            ctrl & FSEL_MASK,
            FSEL_GPIO,
            "FUNCSEL should be GPIO (4) for Input mode"
        );

        // RIO_OE_CLR for bank 0 should have bit 17 written
        let oe_clr_addr = MOCK_BASE + RIO_OE_CLR_OFFSET;
        let oe_clr = mock_mmio::read(oe_clr_addr);
        assert_eq!(
            oe_clr,
            1u32 << 17,
            "RIO_OE_CLR should have bit 17 set for Input mode"
        );
    }

    // ---- Test 2: Set pin to Output mode ----------------------------------

    #[test]
    fn test_set_mode_output() {
        reset_mock();
        let pin = GpioPin::new(4, MOCK_BASE);
        gpio_set_mode(&pin, GpioMode::Output);

        // GPIO_CTRL[4] should have FUNCSEL = 4 (GPIO)
        let ctrl_addr = MOCK_BASE + GPIO_CTRL_OFFSET + 4 * 4;
        let ctrl = mock_mmio::read(ctrl_addr);
        assert_eq!(
            ctrl & FSEL_MASK,
            FSEL_GPIO,
            "FUNCSEL should be GPIO (4) for Output mode"
        );

        // RIO_OE_SET for bank 0 should have bit 4 written
        let oe_set_addr = MOCK_BASE + RIO_OE_SET_OFFSET;
        let oe_set = mock_mmio::read(oe_set_addr);
        assert_eq!(
            oe_set,
            1u32 << 4,
            "RIO_OE_SET should have bit 4 set for Output mode"
        );
    }

    // ---- Test 3: Set pin to AltFunc3 mode --------------------------------

    #[test]
    fn test_set_mode_altfunc() {
        reset_mock();
        let pin = GpioPin::new(18, MOCK_BASE);
        gpio_set_mode(&pin, GpioMode::AltFunc3);

        // GPIO_CTRL[18] should have FUNCSEL = 3 (ALT3)
        let ctrl_addr = MOCK_BASE + GPIO_CTRL_OFFSET + 18 * 4;
        let ctrl = mock_mmio::read(ctrl_addr);
        assert_eq!(
            ctrl & FSEL_MASK,
            FSEL_ALT3,
            "FUNCSEL should be ALT3 (3) for AltFunc3 mode"
        );

        // OE should be cleared for alternate functions
        let oe_clr_addr = MOCK_BASE + RIO_OE_CLR_OFFSET;
        let oe_clr = mock_mmio::read(oe_clr_addr);
        assert_eq!(
            oe_clr,
            1u32 << 18,
            "RIO_OE_CLR should have bit 18 set for AltFunc mode"
        );
    }

    // ---- Test 4: Set pull-up resistor ------------------------------------

    #[test]
    fn test_set_pull_up() {
        reset_mock();
        let pin = GpioPin::new(2, MOCK_BASE);
        gpio_set_pull(&pin, GpioPull::PullUp);

        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 2 * 4;
        let pad = mock_mmio::read(pad_addr);
        let pull_val = (pad & PAD_PULL_MASK) >> PAD_PULL_SHIFT;
        assert_eq!(
            pull_val,
            GpioPull::PullUp as u32,
            "Pull bits should encode PullUp (0b10)"
        );
    }

    // ---- Test 5: Clear pull resistor (None) after PullDown ---------------

    #[test]
    fn test_set_pull_none() {
        reset_mock();
        let pin = GpioPin::new(7, MOCK_BASE);
        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 7 * 4;

        // Pre-populate pad register with PullDown value
        mock_mmio::write(pad_addr, (GpioPull::PullDown as u32) << PAD_PULL_SHIFT);

        // Now set to None — should clear the pull bits
        gpio_set_pull(&pin, GpioPull::None);

        let pad = mock_mmio::read(pad_addr);
        let pull_val = (pad & PAD_PULL_MASK) >> PAD_PULL_SHIFT;
        assert_eq!(
            pull_val,
            GpioPull::None as u32,
            "Pull bits should encode None (0b00) after clearing"
        );
    }

    // ---- Test 6: Write high (RIO_OUT_SET) --------------------------------

    #[test]
    fn test_write_high() {
        reset_mock();
        let pin = GpioPin::new(17, MOCK_BASE);
        gpio_write(&pin, true);

        let set_addr = MOCK_BASE + RIO_OUT_SET_OFFSET;
        let val = mock_mmio::read(set_addr);
        assert_eq!(val, 1u32 << 17, "RIO_OUT_SET should have bit 17 written");

        // RIO_OUT_CLR should NOT have been written (still 0)
        let clr_addr = MOCK_BASE + RIO_OUT_CLR_OFFSET;
        assert_eq!(
            mock_mmio::read(clr_addr),
            0,
            "RIO_OUT_CLR should be untouched"
        );
    }

    // ---- Test 7: Write low (RIO_OUT_CLR) ---------------------------------

    #[test]
    fn test_write_low() {
        reset_mock();
        let pin = GpioPin::new(27, MOCK_BASE);
        gpio_write(&pin, false);

        let clr_addr = MOCK_BASE + RIO_OUT_CLR_OFFSET;
        let val = mock_mmio::read(clr_addr);
        assert_eq!(val, 1u32 << 27, "RIO_OUT_CLR should have bit 27 written");

        // RIO_OUT_SET should NOT have been written
        let set_addr = MOCK_BASE + RIO_OUT_SET_OFFSET;
        assert_eq!(
            mock_mmio::read(set_addr),
            0,
            "RIO_OUT_SET should be untouched"
        );
    }

    // ---- Test 8: Read input from RIO_IN ----------------------------------

    #[test]
    fn test_read_input() {
        reset_mock();
        let pin = GpioPin::new(5, MOCK_BASE);
        let in_addr = MOCK_BASE + RIO_IN_OFFSET;

        // Pre-populate RIO_IN with bit 5 set → read should return true
        mock_mmio::write(in_addr, 1u32 << 5);
        assert!(gpio_read(&pin), "Pin 5 should read high");

        // Clear bit 5 → read should return false
        mock_mmio::write(in_addr, 0);
        assert!(!gpio_read(&pin), "Pin 5 should read low");

        // Set a different bit (bit 3) → pin 5 should still read low
        mock_mmio::write(in_addr, 1u32 << 3);
        assert!(
            !gpio_read(&pin),
            "Pin 5 should read low when only bit 3 is set"
        );
    }

    // ---- Test 9: Toggle output -------------------------------------------

    #[test]
    fn test_toggle_output() {
        reset_mock();
        let pin = GpioPin::new(22, MOCK_BASE);
        let out_addr = MOCK_BASE + RIO_OUT_OFFSET;

        // Start with pin low (RIO_OUT = 0) → toggle should set high
        mock_mmio::write(out_addr, 0);
        pin.toggle();
        // After toggle: RIO_OUT_SET should have bit 22
        let set_addr = MOCK_BASE + RIO_OUT_SET_OFFSET;
        assert_eq!(
            mock_mmio::read(set_addr),
            1u32 << 22,
            "Toggle from low should set high"
        );

        // Now simulate RIO_OUT having bit 22 set → toggle should clear
        reset_mock();
        mock_mmio::write(out_addr, 1u32 << 22);
        pin.toggle();
        let clr_addr = MOCK_BASE + RIO_OUT_CLR_OFFSET;
        assert_eq!(
            mock_mmio::read(clr_addr),
            1u32 << 22,
            "Toggle from high should clear"
        );
    }

    // ---- Test 10: Read output-enable state -------------------------------

    #[test]
    fn test_read_oe() {
        reset_mock();
        let pin = GpioPin::new(17, MOCK_BASE);
        let oe_addr = MOCK_BASE + RIO_OE_OFFSET;

        // OE clear → should return false
        mock_mmio::write(oe_addr, 0);
        assert!(!pin.read_oe(), "OE should be false when cleared");

        // OE with bit 17 → should return true
        mock_mmio::write(oe_addr, 1u32 << 17);
        assert!(pin.read_oe(), "OE should be true when bit 17 set");

        // OE with a different bit → pin 17 should still be false
        mock_mmio::write(oe_addr, 1u32 << 3);
        assert!(!pin.read_oe(), "OE should be false for different bit");
    }

    // ---- Test 11: Read output level (RIO_OUT) ----------------------------

    #[test]
    fn test_read_out() {
        reset_mock();
        let pin = GpioPin::new(17, MOCK_BASE);
        let out_addr = MOCK_BASE + RIO_OUT_OFFSET;

        mock_mmio::write(out_addr, 1u32 << 17);
        assert!(pin.read_out(), "read_out should be true when output high");

        mock_mmio::write(out_addr, 0);
        assert!(!pin.read_out(), "read_out should be false when output low");
    }

    // ---- Test 12: Read mode from GPIO_CTRL -------------------------------

    #[test]
    fn test_read_mode() {
        reset_mock();
        let pin = GpioPin::new(17, MOCK_BASE);
        let ctrl_addr = MOCK_BASE + GPIO_CTRL_OFFSET + 17 * 4;

        // Set to AltFunc2 (FUNCSEL=2)
        mock_mmio::write(ctrl_addr, FSEL_ALT2);
        assert_eq!(pin.read_mode(), FSEL_ALT2, "read_mode should return ALT2");

        // Set to GPIO (FUNCSEL=4)
        mock_mmio::write(ctrl_addr, FSEL_GPIO);
        assert_eq!(pin.read_mode(), FSEL_GPIO, "read_mode should return GPIO");
    }

    // ---- Test 13: Read pull resistor state --------------------------------

    #[test]
    fn test_read_pull() {
        reset_mock();
        let pin = GpioPin::new(10, MOCK_BASE);
        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 10 * 4;

        // Set pull-up
        mock_mmio::write(pad_addr, (GpioPull::PullUp as u32) << PAD_PULL_SHIFT);
        assert_eq!(
            pin.read_pull(),
            GpioPull::PullUp,
            "read_pull should return PullUp"
        );

        // Set pull-down
        mock_mmio::write(pad_addr, (GpioPull::PullDown as u32) << PAD_PULL_SHIFT);
        assert_eq!(
            pin.read_pull(),
            GpioPull::PullDown,
            "read_pull should return PullDown"
        );

        // Set none
        mock_mmio::write(pad_addr, 0);
        assert_eq!(
            pin.read_pull(),
            GpioPull::None,
            "read_pull should return None"
        );
    }

    // ---- Test 14: Bank-1 pin (pin >= 32) addressing ----------------------

    #[test]
    fn test_bank1_pin_addressing() {
        reset_mock();
        let pin = GpioPin::new(40, MOCK_BASE);
        gpio_set_mode(&pin, GpioMode::Output);

        // GPIO_CTRL[40] should have FSEL = 4 (GPIO)
        let ctrl_addr = MOCK_BASE + GPIO_CTRL_OFFSET + 40 * 4;
        let ctrl = mock_mmio::read(ctrl_addr);
        assert_eq!(
            ctrl & FSEL_MASK,
            FSEL_GPIO,
            "FUNCSEL should be GPIO for bank-1 pin"
        );

        // RIO_OE_SET for bank 1: bit 8 (40 - 32 = 8)
        let oe_set_addr = MOCK_BASE + RIO_OE_SET_OFFSET + RIO_BANK_STRIDE;
        let oe_set = mock_mmio::read(oe_set_addr);
        assert_eq!(
            oe_set,
            1u32 << 8,
            "RIO_OE_SET bank 1 should have bit 8 set (pin 40)"
        );
    }

    // ---- Test 15: PWM enable and disable ----------------------------------

    #[test]
    fn test_pwm_enable_disable() {
        reset_mock();
        let pwm = GpioPwm::new(PwmChannel::Pwm0, MOCK_PWM0_BASE);

        // Initially disabled (register default = 0)
        assert!(!pwm.is_enabled(), "PWM should start disabled");

        // Enable
        pwm.enable();
        assert!(pwm.is_enabled(), "PWM should be enabled after enable()");
        let ctrl = mock_mmio::read(MOCK_PWM0_BASE + PWM_CTRL_OFFSET);
        assert_eq!(
            ctrl & PWM_CTRL_ENABLE,
            PWM_CTRL_ENABLE,
            "CTRL register should have ENABLE bit set"
        );

        // Disable
        pwm.disable();
        assert!(!pwm.is_enabled(), "PWM should be disabled after disable()");
        let ctrl = mock_mmio::read(MOCK_PWM0_BASE + PWM_CTRL_OFFSET);
        assert_eq!(
            ctrl & PWM_CTRL_ENABLE,
            0,
            "CTRL register should have ENABLE bit cleared"
        );
    }

    // ---- Test 16: PWM duty cycle and range --------------------------------

    #[test]
    fn test_pwm_duty_and_range() {
        reset_mock();
        let pwm = GpioPwm::new(PwmChannel::Pwm1, MOCK_PWM1_BASE);

        pwm.set_range(1024);
        pwm.set_duty_cycle(512);

        assert_eq!(pwm.range(), 1024, "Range should be 1024");
        assert_eq!(pwm.duty_cycle(), 512, "Duty cycle should be 512");

        // Verify raw register values
        assert_eq!(
            mock_mmio::read(MOCK_PWM1_BASE + PWM_RNG_OFFSET),
            1024,
            "PWM_RNG register should be 1024"
        );
        assert_eq!(
            mock_mmio::read(MOCK_PWM1_BASE + PWM_DAT_OFFSET),
            512,
            "PWM_DAT register should be 512"
        );
    }

    // ---- Test 17: PWM MSEN mode ------------------------------------------

    #[test]
    fn test_pwm_msen_mode() {
        reset_mock();
        let pwm = GpioPwm::new(PwmChannel::Pwm0, MOCK_PWM0_BASE);

        assert!(!pwm.is_msen(), "MSEN should start disabled");

        pwm.enable_msen();
        assert!(pwm.is_msen(), "MSEN should be enabled after enable_msen()");

        pwm.disable_msen();
        assert!(
            !pwm.is_msen(),
            "MSEN should be disabled after disable_msen()"
        );
    }

    // ---- Test 18: GpioPin stores pin number and base ---------------------

    #[test]
    fn test_gpio_pin_number() {
        let pin = GpioPin::new(42, MOCK_BASE);
        assert_eq!(pin.pin(), 42, "Pin number should be 42");
        assert_eq!(pin.base(), MOCK_BASE, "Base address should match");
    }

    // ---- Test 19: RP1 base addresses from platform -----------------------

    #[test]
    fn test_rp1_base_addresses_from_platform() {
        let platform = Pi5Platform::default();
        assert_eq!(
            platform.rp1_gpio_base(),
            RP1_GPIO_BASE,
            "RP1 GPIO base should match constant"
        );
        assert_eq!(
            platform.rp1_pwm0_base(),
            RP1_PWM0_BASE,
            "RP1 PWM0 base should match constant"
        );
        assert_eq!(
            platform.rp1_pwm1_base(),
            RP1_PWM1_BASE,
            "RP1 PWM1 base should match constant"
        );
    }

    // ---- Test 20: All GpioMode FUNCSEL encodings -------------------------

    #[test]
    fn test_gpio_mode_funcsels_encoding() {
        assert_eq!(GpioMode::Input.funcsels(), FSEL_GPIO); // 4
        assert_eq!(GpioMode::Output.funcsels(), FSEL_GPIO); // 4
        assert_eq!(GpioMode::AltFunc0.funcsels(), FSEL_ALT0); // 0
        assert_eq!(GpioMode::AltFunc1.funcsels(), FSEL_ALT1); // 1
        assert_eq!(GpioMode::AltFunc2.funcsels(), FSEL_ALT2); // 2
        assert_eq!(GpioMode::AltFunc3.funcsels(), FSEL_ALT3); // 3
        assert_eq!(GpioMode::AltFunc4.funcsels(), FSEL_ALT4); // 5
        assert_eq!(GpioMode::AltFunc5.funcsels(), FSEL_ALT5); // 6
    }

    // ---- Test 21: GpioPull encoding values --------------------------------

    #[test]
    fn test_gpio_pull_encoding() {
        assert_eq!(GpioPull::None as u32, 0b00);
        assert_eq!(GpioPull::PullDown as u32, 0b01);
        assert_eq!(GpioPull::PullUp as u32, 0b10);
    }

    // ---- Test 22: GpioMode is_output helper ------------------------------

    #[test]
    fn test_gpio_mode_is_output() {
        assert!(GpioMode::Output.is_output());
        assert!(!GpioMode::Input.is_output());
        assert!(!GpioMode::AltFunc0.is_output());
        assert!(!GpioMode::AltFunc5.is_output());
    }

    // ---- Test 23: PWM GPIO pin constants ---------------------------------

    #[test]
    fn test_pwm_gpio_pins() {
        assert_eq!(PWM_GPIO_PINS, [12, 13, 18, 19]);
    }

    // ---- Test 24: Set mode preserves existing GPIO_CTRL bits --------------

    #[test]
    fn test_set_mode_preserves_ctrl_bits() {
        reset_mock();
        let pin = GpioPin::new(3, MOCK_BASE);
        let ctrl_addr = MOCK_BASE + GPIO_CTRL_OFFSET + 3 * 4;

        // Pre-set some other bits in the control register (e.g., OUTOVER)
        let initial_val: u32 = (0b10 << OUTOVER_SHIFT) | (0b11 << OEOVER_SHIFT) | FSEL_ALT2;
        mock_mmio::write(ctrl_addr, initial_val);

        // Set mode to Output — should only change FSEL, not OUTOVER/OEOVER
        pin.set_mode(GpioMode::Output);

        let ctrl = mock_mmio::read(ctrl_addr);
        // FSEL should now be GPIO (4)
        assert_eq!(ctrl & FSEL_MASK, FSEL_GPIO);
        // OUTOVER should be preserved
        assert_eq!(
            (ctrl >> OUTOVER_SHIFT) & 0x3,
            0b10,
            "OUTOVER should be preserved"
        );
        // OEOVER should be preserved
        assert_eq!(
            (ctrl >> OEOVER_SHIFT) & 0x3,
            0b11,
            "OEOVER should be preserved"
        );
    }

    // ---- Test 25: Set pull preserves existing PAD_CTRL bits ---------------

    #[test]
    fn test_set_pull_preserves_pad_bits() {
        reset_mock();
        let pin = GpioPin::new(10, MOCK_BASE);
        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 10 * 4;

        // Pre-set some bits outside the pull field (e.g., bits 0–21)
        let other_bits: u32 = 0x00AB_CD00; // bits outside [23:22]
        mock_mmio::write(
            pad_addr,
            other_bits | (GpioPull::PullDown as u32) << PAD_PULL_SHIFT,
        );

        pin.set_pull(GpioPull::PullUp);

        let pad = mock_mmio::read(pad_addr);
        // Pull bits should now be PullUp
        let pull_val = (pad & PAD_PULL_MASK) >> PAD_PULL_SHIFT;
        assert_eq!(pull_val, GpioPull::PullUp as u32);
        // Other bits should be preserved
        assert_eq!(
            pad & !PAD_PULL_MASK,
            other_bits & !PAD_PULL_MASK,
            "Non-pull bits should be preserved"
        );
    }

    // ---- Test 26: 40-pin header mapping ----------------------------------

    #[test]
    fn test_header_pin_mapping() {
        // Pin 1 = 3V3 power
        assert_eq!(header_pin(1).unwrap().kind, HeaderPinKind::Power3V3);
        // Pin 2 = 5V power
        assert_eq!(header_pin(2).unwrap().kind, HeaderPinKind::Power5V);
        // Pin 3 = GPIO2 (SDA1)
        assert_eq!(header_pin(3).unwrap().kind, HeaderPinKind::Gpio(2));
        // Pin 6 = GND
        assert_eq!(header_pin(6).unwrap().kind, HeaderPinKind::Ground);
        // Pin 8 = GPIO14 (TXD0)
        assert_eq!(header_pin(8).unwrap().kind, HeaderPinKind::Gpio(14));
        // Pin 12 = GPIO18 (PWM0)
        assert_eq!(header_pin(12).unwrap().kind, HeaderPinKind::Gpio(18));
        // Pin 32 = GPIO12 (PWM0)
        assert_eq!(header_pin(32).unwrap().kind, HeaderPinKind::Gpio(12));
        // Pin 33 = GPIO13 (PWM1)
        assert_eq!(header_pin(33).unwrap().kind, HeaderPinKind::Gpio(13));
        // Pin 35 = GPIO19 (PWM1)
        assert_eq!(header_pin(35).unwrap().kind, HeaderPinKind::Gpio(19));
        // Pin 40 = GPIO21
        assert_eq!(header_pin(40).unwrap().kind, HeaderPinKind::Gpio(21));
    }

    // ---- Test 27: Header pin out of range --------------------------------

    #[test]
    fn test_header_pin_out_of_range() {
        assert!(header_pin(0).is_none(), "Pin 0 should be out of range");
        assert!(header_pin(41).is_none(), "Pin 41 should be out of range");
        assert!(header_pin(255).is_none(), "Pin 255 should be out of range");
    }

    // ---- Test 28: gpio_from_header ---------------------------------------

    #[test]
    fn test_gpio_from_header() {
        // Power and ground pins should return None
        assert_eq!(gpio_from_header(1), None, "3V3 pin should return None");
        assert_eq!(gpio_from_header(2), None, "5V pin should return None");
        assert_eq!(gpio_from_header(6), None, "GND pin should return None");

        // GPIO pins should return the GPIO number
        assert_eq!(gpio_from_header(3), Some(2), "Pin 3 → GPIO2");
        assert_eq!(gpio_from_header(11), Some(17), "Pin 11 → GPIO17");
        assert_eq!(gpio_from_header(12), Some(18), "Pin 12 → GPIO18");
        assert_eq!(gpio_from_header(32), Some(12), "Pin 32 → GPIO12");
    }

    // ---- Test 29: GpioPin::from_header -----------------------------------

    #[test]
    fn test_gpio_pin_from_header() {
        // GPIO pin → Some(GpioPin)
        let pin = GpioPin::from_header(11, MOCK_BASE);
        assert!(pin.is_some(), "Pin 11 should yield a GpioPin");
        let pin = pin.unwrap();
        assert_eq!(pin.pin(), 17, "Pin 11 → GPIO17");
        assert_eq!(pin.base(), MOCK_BASE);

        // Power pin → None
        assert!(
            GpioPin::from_header(1, MOCK_BASE).is_none(),
            "Power pin should return None"
        );

        // Ground pin → None
        assert!(
            GpioPin::from_header(6, MOCK_BASE).is_none(),
            "Ground pin should return None"
        );
    }

    // ---- Test 30: PWM alt function mapping -------------------------------

    #[test]
    fn test_pwm_alt_func_mapping() {
        assert_eq!(pwm_alt_func_for_gpio(12), Some(GpioMode::AltFunc0));
        assert_eq!(pwm_alt_func_for_gpio(13), Some(GpioMode::AltFunc0));
        assert_eq!(pwm_alt_func_for_gpio(18), Some(GpioMode::AltFunc5));
        assert_eq!(pwm_alt_func_for_gpio(19), Some(GpioMode::AltFunc5));
        assert_eq!(pwm_alt_func_for_gpio(17), None, "GPIO17 has no PWM");
        assert_eq!(pwm_alt_func_for_gpio(4), None, "GPIO4 has no PWM");
    }

    // ---- Test 31: PwmChannel::gpio_pins ----------------------------------

    #[test]
    fn test_pwm_channel_gpio_pins() {
        assert_eq!(PwmChannel::Pwm0.gpio_pins(), [12, 18]);
        assert_eq!(PwmChannel::Pwm1.gpio_pins(), [13, 19]);
    }

    // ---- Test 32: GpioPwm::from_gpio -------------------------------------

    #[test]
    fn test_gpio_pwm_from_gpio() {
        assert_eq!(
            GpioPwm::from_gpio(12, MOCK_PWM0_BASE).unwrap().channel(),
            PwmChannel::Pwm0
        );
        assert_eq!(
            GpioPwm::from_gpio(18, MOCK_PWM0_BASE).unwrap().channel(),
            PwmChannel::Pwm0
        );
        assert_eq!(
            GpioPwm::from_gpio(13, MOCK_PWM1_BASE).unwrap().channel(),
            PwmChannel::Pwm1
        );
        assert_eq!(
            GpioPwm::from_gpio(19, MOCK_PWM1_BASE).unwrap().channel(),
            PwmChannel::Pwm1
        );
        assert!(
            GpioPwm::from_gpio(17, MOCK_PWM0_BASE).is_none(),
            "GPIO17 should not support PWM"
        );
    }

    // ---- Test 33: Header pin default functions ----------------------------

    #[test]
    fn test_header_pin_default_funcs() {
        assert_eq!(header_pin(3).unwrap().default_func, "SDA1");
        assert_eq!(header_pin(5).unwrap().default_func, "SCL1");
        assert_eq!(header_pin(8).unwrap().default_func, "TXD0");
        assert_eq!(header_pin(10).unwrap().default_func, "RXD0");
        assert_eq!(header_pin(12).unwrap().default_func, "PWM0");
        assert_eq!(header_pin(32).unwrap().default_func, "PWM0");
        assert_eq!(header_pin(33).unwrap().default_func, "PWM1");
        assert_eq!(header_pin(35).unwrap().default_func, "PWM1");
    }

    // ---- Test 34: Bank-1 pin read/write ----------------------------------

    #[test]
    fn test_bank1_pin_read_write() {
        reset_mock();
        let pin = GpioPin::new(40, MOCK_BASE);

        // Write high on bank-1 pin
        pin.write(true);
        let set_addr = MOCK_BASE + RIO_OUT_SET_OFFSET + RIO_BANK_STRIDE;
        assert_eq!(
            mock_mmio::read(set_addr),
            1u32 << 8, // pin 40 - 32 = 8
            "Bank-1 OUT_SET should have bit 8 set"
        );

        // Write low
        reset_mock();
        pin.write(false);
        let clr_addr = MOCK_BASE + RIO_OUT_CLR_OFFSET + RIO_BANK_STRIDE;
        assert_eq!(
            mock_mmio::read(clr_addr),
            1u32 << 8,
            "Bank-1 OUT_CLR should have bit 8 set"
        );
    }

    // ---- Test 35: Header pin count ----------------------------------------

    #[test]
    fn test_header_pin_count() {
        assert_eq!(HEADER_PINS.len(), 40, "There should be 40 header pins");
        // Verify sequential pin numbering
        for (i, hp) in HEADER_PINS.iter().enumerate() {
            assert_eq!(hp.pin as usize, i + 1, "Pin numbering should be sequential");
        }
    }

    // =========================================================================
    // BCM2712 RP1 advanced pad control tests
    // =========================================================================

    // ---- Test 36: Set drive strength to 12 mA ----------------------------

    #[test]
    fn test_set_drive_strength_12ma() {
        reset_mock();
        let pin = GpioPin::new(17, MOCK_BASE);
        gpio_set_drive_strength(&pin, DriveStrength::Ma12);

        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 17 * 4;
        let pad = mock_mmio::read(pad_addr);
        let drive_val = (pad & PAD_DRIVE_MASK) >> PAD_DRIVE_SHIFT;
        assert_eq!(
            drive_val,
            DriveStrength::Ma12 as u32,
            "Drive bits should encode Ma12 (0b11)"
        );
    }

    // ---- Test 37: Drive strength preserves other PAD_CTRL bits -----------

    #[test]
    fn test_drive_strength_preserves_pad_bits() {
        reset_mock();
        let pin = GpioPin::new(3, MOCK_BASE);
        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 3 * 4;

        // Pre-set pull-up + Schmitt + input enable + some other bits
        let initial: u32 = (GpioPull::PullUp as u32) << PAD_PULL_SHIFT
            | PAD_SCHMITT_BIT
            | PAD_IN_ENABLE_BIT
            | (DriveStrength::Ma2 as u32);
        mock_mmio::write(pad_addr, initial);

        gpio_set_drive_strength(&pin, DriveStrength::Ma8);

        let pad = mock_mmio::read(pad_addr);
        // Drive should now be Ma8
        assert_eq!(
            (pad & PAD_DRIVE_MASK) >> PAD_DRIVE_SHIFT,
            DriveStrength::Ma8 as u32,
            "Drive should be Ma8"
        );
        // Pull should be preserved
        assert_eq!(
            (pad & PAD_PULL_MASK) >> PAD_PULL_SHIFT,
            GpioPull::PullUp as u32,
            "Pull should be preserved"
        );
        // Schmitt and input enable should be preserved
        assert_eq!(
            pad & PAD_SCHMITT_BIT,
            PAD_SCHMITT_BIT,
            "Schmitt should be preserved"
        );
        assert_eq!(
            pad & PAD_IN_ENABLE_BIT,
            PAD_IN_ENABLE_BIT,
            "Input enable should be preserved"
        );
    }

    // ---- Test 38: Read drive strength round-trip --------------------------

    #[test]
    fn test_read_drive_strength_round_trip() {
        reset_mock();
        let pin = GpioPin::new(5, MOCK_BASE);

        for strength in [
            DriveStrength::Ma2,
            DriveStrength::Ma4,
            DriveStrength::Ma8,
            DriveStrength::Ma12,
        ] {
            pin.set_drive_strength(strength);
            assert_eq!(
                pin.read_drive_strength(),
                strength,
                "read_drive_strength should match set value {:?}",
                strength
            );
        }
    }

    // ---- Test 39: Input enable on/off ------------------------------------

    #[test]
    fn test_input_enable_on_off() {
        reset_mock();
        let pin = GpioPin::new(22, MOCK_BASE);
        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 22 * 4;

        // Enable input
        gpio_set_input_enable(&pin, true);
        let pad = mock_mmio::read(pad_addr);
        assert_eq!(
            pad & PAD_IN_ENABLE_BIT,
            PAD_IN_ENABLE_BIT,
            "Input enable bit should be set"
        );
        assert!(
            pin.read_input_enable(),
            "read_input_enable should return true"
        );

        // Disable input
        gpio_set_input_enable(&pin, false);
        let pad = mock_mmio::read(pad_addr);
        assert_eq!(
            pad & PAD_IN_ENABLE_BIT,
            0,
            "Input enable bit should be cleared"
        );
        assert!(
            !pin.read_input_enable(),
            "read_input_enable should return false"
        );
    }

    // ---- Test 40: Schmitt trigger on/off ---------------------------------

    #[test]
    fn test_schmitt_trigger_on_off() {
        reset_mock();
        let pin = GpioPin::new(4, MOCK_BASE);
        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 4 * 4;

        // Enable Schmitt
        gpio_set_schmitt(&pin, true);
        let pad = mock_mmio::read(pad_addr);
        assert_eq!(
            pad & PAD_SCHMITT_BIT,
            PAD_SCHMITT_BIT,
            "Schmitt bit should be set"
        );
        assert!(pin.read_schmitt(), "read_schmitt should return true");

        // Disable Schmitt
        gpio_set_schmitt(&pin, false);
        let pad = mock_mmio::read(pad_addr);
        assert_eq!(pad & PAD_SCHMITT_BIT, 0, "Schmitt bit should be cleared");
        assert!(!pin.read_schmitt(), "read_schmitt should return false");
    }

    // ---- Test 41: Slew rate control --------------------------------------

    #[test]
    fn test_slew_rate_control() {
        reset_mock();
        let pin = GpioPin::new(18, MOCK_BASE);
        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 18 * 4;

        // Enable fast slew
        gpio_set_slew_fast(&pin, true);
        let pad = mock_mmio::read(pad_addr);
        assert_eq!(
            pad & PAD_SLEW_FAST_BIT,
            PAD_SLEW_FAST_BIT,
            "Slew fast bit should be set"
        );
        assert!(pin.read_slew_fast(), "read_slew_fast should return true");

        // Disable fast slew (slow mode)
        gpio_set_slew_fast(&pin, false);
        let pad = mock_mmio::read(pad_addr);
        assert_eq!(
            pad & PAD_SLEW_FAST_BIT,
            0,
            "Slew fast bit should be cleared"
        );
        assert!(!pin.read_slew_fast(), "read_slew_fast should return false");
    }

    // ---- Test 42: Combined pad configuration (pull + drive + Schmitt) ----

    #[test]
    fn test_combined_pad_configuration() {
        reset_mock();
        let pin = GpioPin::new(17, MOCK_BASE);

        // Configure all pad properties together
        gpio_set_mode(&pin, GpioMode::Output);
        gpio_set_pull(&pin, GpioPull::PullUp);
        gpio_set_drive_strength(&pin, DriveStrength::Ma8);
        gpio_set_schmitt(&pin, true);
        gpio_set_input_enable(&pin, true);
        gpio_set_slew_fast(&pin, true);

        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 17 * 4;
        let pad = mock_mmio::read(pad_addr);

        // Verify all fields
        assert_eq!(
            (pad & PAD_PULL_MASK) >> PAD_PULL_SHIFT,
            GpioPull::PullUp as u32,
            "Pull should be PullUp"
        );
        assert_eq!(
            (pad & PAD_DRIVE_MASK) >> PAD_DRIVE_SHIFT,
            DriveStrength::Ma8 as u32,
            "Drive should be Ma8"
        );
        assert_eq!(
            pad & PAD_SCHMITT_BIT,
            PAD_SCHMITT_BIT,
            "Schmitt should be on"
        );
        assert_eq!(
            pad & PAD_IN_ENABLE_BIT,
            PAD_IN_ENABLE_BIT,
            "Input should be enabled"
        );
        assert_eq!(
            pad & PAD_SLEW_FAST_BIT,
            PAD_SLEW_FAST_BIT,
            "Slew should be fast"
        );
    }

    // ---- Test 43: DriveStrength enum repr values -------------------------

    #[test]
    fn test_drive_strength_encoding() {
        assert_eq!(DriveStrength::Ma2 as u32, 0b00);
        assert_eq!(DriveStrength::Ma4 as u32, 0b01);
        assert_eq!(DriveStrength::Ma8 as u32, 0b10);
        assert_eq!(DriveStrength::Ma12 as u32, 0b11);
    }

    // ---- Test 44: Input enable preserves other bits -----------------------

    #[test]
    fn test_input_enable_preserves_other_bits() {
        reset_mock();
        let pin = GpioPin::new(10, MOCK_BASE);
        let pad_addr = MOCK_BASE + PAD_CTRL_OFFSET + 10 * 4;

        // Pre-set all pad control bits
        let initial: u32 = (GpioPull::PullDown as u32) << PAD_PULL_SHIFT
            | (DriveStrength::Ma12 as u32)
            | PAD_SCHMITT_BIT
            | PAD_SLEW_FAST_BIT;
        mock_mmio::write(pad_addr, initial);

        // Disable input
        pin.set_input_enable(false);
        let pad = mock_mmio::read(pad_addr);
        assert_eq!(pad & PAD_IN_ENABLE_BIT, 0, "Input should be disabled");
        assert_eq!(
            (pad & PAD_PULL_MASK) >> PAD_PULL_SHIFT,
            GpioPull::PullDown as u32,
            "Pull should be preserved"
        );
        assert_eq!(
            (pad & PAD_DRIVE_MASK) >> PAD_DRIVE_SHIFT,
            DriveStrength::Ma12 as u32,
            "Drive should be preserved"
        );
        assert_eq!(pad & PAD_SCHMITT_BIT, PAD_SCHMITT_BIT, "Schmitt preserved");
        assert_eq!(pad & PAD_SLEW_FAST_BIT, PAD_SLEW_FAST_BIT, "Slew preserved");
    }
}
