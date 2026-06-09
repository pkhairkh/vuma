//! UART (PL011) serial access for the Raspberry Pi 5.
//!
//! Provides the [`Uart`] struct for initialising, reading from, and writing
//! to the BCM2712's PL011 UART. The register layout follows the ARM
//! PrimeCell PL011 TRM.

use crate::mmio::{mmio_read, mmio_write, Address};

// ---------------------------------------------------------------------------
// PL011 register offsets (relative to UART base)
// ---------------------------------------------------------------------------

/// Data Register — reads/writes the top of the TX/RX FIFO.
pub const DR: usize = 0x00;

/// Receive Status Register / Error Clear Register.
pub const RSR_ECR: usize = 0x04;

/// Flag Register — contains TXFF, RXFE, BUSY bits, etc.
pub const FR: usize = 0x18;

/// Fractional Baud Rate Divider.
pub const FBRD: usize = 0x28;

/// Integer Baud Rate Divider.
pub const IBRD: usize = 0x24;

/// Line Control Register (data length, parity, stop bits, FIFO enable).
pub const LCRH: usize = 0x2C;

/// Control Register (UART enable, TX/RX enable).
pub const CR: usize = 0x30;

/// Interrupt FIFO Level Select.
pub const IFLS: usize = 0x34;

/// Interrupt Mask Set/Clear.
pub const IMSC: usize = 0x38;

/// Raw Interrupt Status.
pub const RIS: usize = 0x3C;

/// Masked Interrupt Status.
pub const MIS: usize = 0x40;

/// Interrupt Clear.
pub const ICR: usize = 0x44;

// ---------------------------------------------------------------------------
// Flag Register bits
// ---------------------------------------------------------------------------

/// TX FIFO full bit in FR.
pub const FR_TXFF: u32 = 1 << 5;
/// RX FIFO empty bit in FR.
pub const FR_RXFE: u32 = 1 << 4;
/// UART busy bit in FR.
pub const FR_BUSY: u32 = 1 << 3;

// ---------------------------------------------------------------------------
// Control Register bits
// ---------------------------------------------------------------------------

/// UART enable bit in CR.
pub const CR_UARTEN: u32 = 1 << 0;
/// TX enable bit in CR.
pub const CR_TXE: u32 = 1 << 8;
/// RX enable bit in CR.
pub const CR_RXE: u32 = 1 << 9;

// ---------------------------------------------------------------------------
// LCRH bits
// ---------------------------------------------------------------------------

/// Enable FIFOs bit in LCRH.
pub const LCRH_FEN: u32 = 1 << 4;
/// 8-bit word length (bits 5:6 = 11).
pub const LCRH_WLEN8: u32 = 3 << 5;

// ---------------------------------------------------------------------------
// Uart
// ---------------------------------------------------------------------------

/// A handle to a PL011 UART on the BCM2712.
///
/// The Pi 5 exposes at least one PL011 (UART0) mapped at the standard
/// peripheral offset. Additional UART instances (UART1–UART5 on the
/// BCM2712) can be accessed by passing the appropriate base address.
#[derive(Debug)]
pub struct Uart {
    /// Base address of the PL011 register block.
    base: Address,
}

impl Uart {
    /// Creates a new UART handle at the given base address.
    #[inline]
    pub const fn new(base: Address) -> Self {
        Self { base }
    }

    /// Returns the base address.
    #[inline]
    pub const fn base(&self) -> Address {
        self.base
    }

    /// Initialises the UART with the specified baud rate.
    ///
    /// Assumes the UART input clock is 48 MHz (the default on the Pi 5).
    /// Configures 8 data bits, no parity, 1 stop bit, and enables FIFOs.
    ///
    /// # Arguments
    ///
    /// * `baud_rate` — Desired baud rate (e.g. 115200).
    pub fn init(&self, baud_rate: u32) {
        // Disable the UART.
        mmio_write(self.base + CR, 0);

        // Clear pending interrupts.
        mmio_write(self.base + ICR, 0x7FF);

        // Calculate baud rate dividers.
        // BAUDDIV = UARTCLK / (16 * BaudRate)
        // IBRD = integer part, FBRD = fractional part * 64 + 0.5
        let uart_clock: u64 = 48_000_000;
        let baud_divider = uart_clock / (16 * baud_rate as u64);
        let fractional = ((uart_clock % (16 * baud_rate as u64)) * 64
            + 8 * baud_rate as u64)
            / (16 * baud_rate as u64);

        mmio_write(self.base + IBRD, baud_divider as u32);
        mmio_write(self.base + FBRD, fractional as u32);

        // 8 data bits, no parity, 1 stop bit, enable FIFOs.
        mmio_write(self.base + LCRH, LCRH_WLEN8 | LCRH_FEN);

        // Disable all interrupts by masking.
        mmio_write(self.base + IMSC, 0);

        // Enable UART, TX, and RX.
        mmio_write(self.base + CR, CR_UARTEN | CR_TXE | CR_RXE);
    }

    /// Writes a single byte to the UART, blocking until the TX FIFO
    /// has space.
    #[inline]
    pub fn write_byte(&self, byte: u8) {
        // Wait until TX FIFO is not full.
        while (mmio_read(self.base + FR) & FR_TXFF) != 0 {
            core::hint::spin_loop();
        }
        mmio_write(self.base + DR, byte as u32);
    }

    /// Reads a single byte from the UART, blocking until the RX FIFO
    /// is non-empty.
    #[inline]
    pub fn read_byte(&self) -> u8 {
        // Wait until RX FIFO is not empty.
        while (mmio_read(self.base + FR) & FR_RXFE) != 0 {
            core::hint::spin_loop();
        }
        mmio_read(self.base + DR) as u8
    }

    /// Writes a string slice to the UART, byte by byte.
    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            // Convert `\n` to `\r\n` for terminal compatibility.
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }

    /// Returns `true` if at least one byte is available in the RX FIFO.
    #[inline]
    pub fn available(&self) -> bool {
        (mmio_read(self.base + FR) & FR_RXFE) == 0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_stores_base_address() {
        let uart = Uart::new(0x1C01_1000);
        assert_eq!(uart.base(), 0x1C01_1000);
    }
}
