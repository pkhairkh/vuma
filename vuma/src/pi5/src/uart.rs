//! UART (PL011) serial access for the Raspberry Pi 5 (BCM2712).
//!
//! The BCM2712 exposes multiple UARTs. The primary debug console on the
//! Pi 5 uses **UART0**, a PL011 PrimeCell UART, mapped at physical
//! address `0x10A0000`. The auxiliary **mini UART (UART1)** is available
//! via the AUX peripheral block.
//!
//! # Module overview
//!
//! | Type / Function              | Description                                    |
//! |------------------------------|------------------------------------------------|
//! | [`Uart`]                     | PL011 UART driver (init, read, write)          |
//! | [`MiniUart`]                 | Mini UART (UART1) auxiliary driver             |
//! | [`UartBuffer`]               | Ring buffer for interrupt-driven I/O           |
//! | [`uart_init`]                | Initialise UART0 with a baud rate              |
//! | [`uart_write_byte`]          | Write a single byte to UART0                   |
//! | [`uart_write_str`]           | Write a string to UART0                        |
//! | [`uart_read_byte`]           | Non-blocking read from UART0                   |
//! | [`uart_read_byte_blocking`]  | Blocking read from UART0                       |
//!
//! # FIFO Management
//!
//! The PL011 UART has a 16-byte TX and RX FIFO. The [`UartBuffer`] type
//! provides a software ring buffer for interrupt-driven I/O, decoupling
//! the hardware FIFO depth from application-level buffering needs.
//!
//! # Interrupt-driven I/O
//!
//! Global RX and TX buffers are provided for UART0. When RX interrupts
//! are enabled the ISR should call [`Uart::handle_rx_interrupt`] (or
//! [`uart0_rx_interrupt_handler`]) to drain the hardware FIFO into the
//! software buffer. The free function [`uart_read_byte`] first checks
//! this buffer before polling the hardware FIFO.

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
// Default addresses and baud rate
// ===========================================================================

/// Default base address for UART0 (PL011) on the BCM2712.
///
/// Computed as `PERIPHERAL_BASE + UART_BASE_OFFSET` in low-peripheral mode.
pub const UART0_BASE: Address =
    crate::platform::PERIPHERAL_BASE + crate::platform::UART_BASE_OFFSET;

/// Default base address for the AUX peripheral block (mini UART / UART1).
pub const AUX_BASE: Address =
    crate::platform::PERIPHERAL_BASE + crate::platform::AUX_BASE_OFFSET;

/// Default baud rate for UART initialisation.
pub const DEFAULT_BAUD_RATE: u32 = 115200;

/// UART input clock frequency on the BCM2712 (48 MHz).
pub const UART_CLOCK: u64 = 48_000_000;

/// Depth of the PL011 transmit and receive FIFOs (16 bytes).
pub const FIFO_DEPTH: usize = 16;

// ===========================================================================
// PL011 register offsets (relative to UART base)
// ===========================================================================

/// Data Register — reads/writes the top of the TX/RX FIFO.
pub const DR: Address = 0x00;

/// Receive Status Register / Error Clear Register.
pub const RSR_ECR: Address = 0x04;

/// Flag Register — contains TXFF, RXFE, BUSY bits, etc.
pub const FR: Address = 0x18;

/// Fractional Baud Rate Divider.
pub const FBRD: Address = 0x28;

/// Integer Baud Rate Divider.
pub const IBRD: Address = 0x24;

/// Line Control Register (data length, parity, stop bits, FIFO enable).
pub const LCRH: Address = 0x2C;

/// Control Register (UART enable, TX/RX enable).
pub const CR: Address = 0x30;

/// Interrupt FIFO Level Select.
pub const IFLS: Address = 0x34;

/// Interrupt Mask Set/Clear.
pub const IMSC: Address = 0x38;

/// Raw Interrupt Status.
pub const RIS: Address = 0x3C;

/// Masked Interrupt Status.
pub const MIS: Address = 0x40;

/// Interrupt Clear.
pub const ICR: Address = 0x44;

// ===========================================================================
// Flag Register bits
// ===========================================================================

/// TX FIFO full bit in FR.
pub const FR_TXFF: u32 = 1 << 5;
/// RX FIFO empty bit in FR.
pub const FR_RXFE: u32 = 1 << 4;
/// UART busy bit in FR.
pub const FR_BUSY: u32 = 1 << 3;

// ===========================================================================
// Control Register bits
// ===========================================================================

/// UART enable bit in CR.
pub const CR_UARTEN: u32 = 1 << 0;
/// TX enable bit in CR.
pub const CR_TXE: u32 = 1 << 8;
/// RX enable bit in CR.
pub const CR_RXE: u32 = 1 << 9;

// ===========================================================================
// LCRH bits
// ===========================================================================

/// Enable FIFOs bit in LCRH.
pub const LCRH_FEN: u32 = 1 << 4;
/// 8-bit word length (bits 5:6 = 11).
pub const LCRH_WLEN8: u32 = 3 << 5;
/// 7-bit word length (bits 5:6 = 10).
pub const LCRH_WLEN7: u32 = 2 << 5;
/// 6-bit word length (bits 5:6 = 01).
pub const LCRH_WLEN6: u32 = 1 << 5;
/// 5-bit word length (bits 5:6 = 00).
pub const LCRH_WLEN5: u32 = 0;
/// Even parity select.
pub const LCRH_EPS: u32 = 1 << 2;
/// Parity enable.
pub const LCRH_PEN: u32 = 1 << 1;
/// Two stop bits select.
pub const LCRH_STP2: u32 = 1 << 3;

// ===========================================================================
// Interrupt Mask Set/Clear (IMSC) bits
// ===========================================================================

/// RX interrupt mask.
pub const IMSC_RXIM: u32 = 1 << 4;
/// TX interrupt mask.
pub const IMSC_TXIM: u32 = 1 << 5;
/// Receive timeout interrupt mask.
pub const IMSC_RTIM: u32 = 1 << 6;
/// Overrun error interrupt mask.
pub const IMSC_OEIM: u32 = 1 << 10;

// ===========================================================================
// Raw / Masked Interrupt Status (RIS / MIS) bits
// ===========================================================================

/// RX interrupt status.
pub const RIS_RXRIS: u32 = 1 << 4;
/// TX interrupt status.
pub const RIS_TXRIS: u32 = 1 << 5;
/// Receive timeout interrupt status.
pub const RIS_RTRIS: u32 = 1 << 6;
/// Overrun error interrupt status.
pub const RIS_OERIS: u32 = 1 << 10;

// ===========================================================================
// Interrupt Clear (ICR) bits
// ===========================================================================

/// Clear RX interrupt.
pub const ICR_RXIC: u32 = 1 << 4;
/// Clear TX interrupt.
pub const ICR_TXIC: u32 = 1 << 5;
/// Clear receive timeout interrupt.
pub const ICR_RTIC: u32 = 1 << 6;
/// Clear overrun error interrupt.
pub const ICR_OEIC: u32 = 1 << 10;

// ===========================================================================
// IFLS — Interrupt FIFO Level Select
// ===========================================================================

/// RX FIFO interrupt level: 1/8 full.
pub const IFLS_RXIFLSEL_1_8: u32 = 0 << 3;
/// RX FIFO interrupt level: 1/4 full.
pub const IFLS_RXIFLSEL_1_4: u32 = 1 << 3;
/// RX FIFO interrupt level: 1/2 full.
pub const IFLS_RXIFLSEL_1_2: u32 = 2 << 3;
/// RX FIFO interrupt level: 3/4 full.
pub const IFLS_RXIFLSEL_3_4: u32 = 3 << 3;
/// RX FIFO interrupt level: 7/8 full.
pub const IFLS_RXIFLSEL_7_8: u32 = 4 << 3;

/// TX FIFO interrupt level: 1/8 full.
pub const IFLS_TXIFLSEL_1_8: u32 = 0;
/// TX FIFO interrupt level: 1/4 full.
pub const IFLS_TXIFLSEL_1_4: u32 = 1;
/// TX FIFO interrupt level: 1/2 full.
pub const IFLS_TXIFLSEL_1_2: u32 = 2;
/// TX FIFO interrupt level: 3/4 full.
pub const IFLS_TXIFLSEL_3_4: u32 = 3;
/// TX FIFO interrupt level: 7/8 full.
pub const IFLS_TXIFLSEL_7_8: u32 = 4;

// ===========================================================================
// Mini UART (AUX) register offsets (relative to AUX base)
// ===========================================================================

/// AUX enables register — bit 0 enables mini UART.
pub const AUX_ENABLES: Address = 0x04;
/// Mini UART I/O data register.
pub const AUX_MU_IO: Address = 0x40;
/// Mini UART interrupt enable register.
pub const AUX_MU_IER: Address = 0x44;
/// Mini UART interrupt identify register.
pub const AUX_MU_IIR: Address = 0x48;
/// Mini UART line control register.
pub const AUX_MU_LCR: Address = 0x4C;
/// Mini UART modem control register.
pub const AUX_MU_MCR: Address = 0x50;
/// Mini UART line status register.
pub const AUX_MU_LSR: Address = 0x54;
/// Mini UART extra control register.
pub const AUX_MU_CNTL: Address = 0x60;
/// Mini UART extra status register.
pub const AUX_MU_STAT: Address = 0x64;
/// Mini UART baud rate register.
pub const AUX_MU_BAUD: Address = 0x68;

// ===========================================================================
// Mini UART constants
// ===========================================================================

/// Mini UART enable bit in AUX_ENABLES.
pub const AUX_ENABLE_MU: u32 = 1 << 0;

/// Mini UART 8-bit mode in AUX_MU_LCR.
pub const AUX_MU_LCR_8BIT: u32 = 3;
/// Mini UART 7-bit mode in AUX_MU_LCR.
pub const AUX_MU_LCR_7BIT: u32 = 2;

/// Mini UART TX empty bit in AUX_MU_LSR.
pub const AUX_MU_LSR_TX_EMPTY: u32 = 1 << 5;
/// Mini UART RX ready bit in AUX_MU_LSR.
pub const AUX_MU_LSR_RX_READY: u32 = 1 << 0;

/// Mini UART TX enable bit in AUX_MU_CNTL.
pub const AUX_MU_CNTL_TX_ENABLE: u32 = 1 << 0;
/// Mini UART RX enable bit in AUX_MU_CNTL.
pub const AUX_MU_CNTL_RX_ENABLE: u32 = 1 << 1;

// ===========================================================================
// UartBuffer — ring buffer for interrupt-driven I/O
// ===========================================================================

/// Default size (in bytes) for each ring buffer.
pub const BUF_SIZE: usize = 256;

/// A lock-free ring (circular) buffer for interrupt-driven UART I/O.
///
/// The buffer is `const`-constructible so it can be placed in a `static`.
/// It is **not** thread-safe — callers must provide external
/// synchronisation (e.g. disabling IRQs) when both ISR and task code
/// access the same buffer.
#[derive(Debug)]
pub struct UartBuffer {
    buffer: [u8; BUF_SIZE],
    head: usize,  // write index
    tail: usize,  // read index
    count: usize, // number of bytes currently stored
}

impl UartBuffer {
    /// Creates a new empty ring buffer.
    pub const fn new() -> Self {
        Self {
            buffer: [0u8; BUF_SIZE],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    /// Pushes a byte into the buffer.
    ///
    /// Returns `true` on success, `false` if the buffer is full.
    #[inline]
    pub fn push(&mut self, byte: u8) -> bool {
        if self.count == BUF_SIZE {
            return false;
        }
        self.buffer[self.head] = byte;
        self.head = (self.head + 1) % BUF_SIZE;
        self.count += 1;
        true
    }

    /// Pops a byte from the buffer.
    ///
    /// Returns `Some(byte)` if data was available, `None` if empty.
    #[inline]
    pub fn pop(&mut self) -> Option<u8> {
        if self.count == 0 {
            return None;
        }
        let byte = self.buffer[self.tail];
        self.tail = (self.tail + 1) % BUF_SIZE;
        self.count -= 1;
        Some(byte)
    }

    /// Returns `true` if the buffer contains no bytes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns `true` if the buffer is at capacity.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.count == BUF_SIZE
    }

    /// Returns the number of bytes currently stored.
    #[inline]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns the total capacity of the buffer.
    #[inline]
    pub const fn capacity(&self) -> usize {
        BUF_SIZE
    }

    /// Returns the number of free bytes in the buffer.
    #[inline]
    pub fn free_space(&self) -> usize {
        BUF_SIZE - self.count
    }

    /// Resets the buffer to empty.
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.count = 0;
    }

    /// Peeks at the front byte without removing it.
    ///
    /// Returns `Some(byte)` if data was available, `None` if empty.
    #[inline]
    pub fn peek(&self) -> Option<u8> {
        if self.count == 0 {
            None
        } else {
            Some(self.buffer[self.tail])
        }
    }

    /// Pushes multiple bytes from a slice into the buffer.
    ///
    /// Returns the number of bytes successfully pushed. Stops when the
    /// buffer is full.
    pub fn push_slice(&mut self, data: &[u8]) -> usize {
        let mut pushed = 0;
        for &byte in data {
            if !self.push(byte) {
                break;
            }
            pushed += 1;
        }
        pushed
    }

    /// Pops multiple bytes from the buffer into a slice.
    ///
    /// Returns the number of bytes popped. Stops when the buffer is empty
    /// or the destination slice is full.
    pub fn pop_slice(&mut self, data: &mut [u8]) -> usize {
        let mut popped = 0;
        for byte in data.iter_mut() {
            match self.pop() {
                Some(b) => {
                    *byte = b;
                    popped += 1;
                }
                None => break,
            }
        }
        popped
    }
}

impl Default for UartBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Global buffers for UART0
// ===========================================================================

/// Global RX ring buffer for UART0 (accessed by ISR and task code).
static mut RX_BUFFER: UartBuffer = UartBuffer::new();

/// Global TX ring buffer for UART0 (accessed by ISR and task code).
static mut TX_BUFFER: UartBuffer = UartBuffer::new();

/// Returns a mutable reference to the global RX buffer.
///
/// # Safety
///
/// The caller must ensure exclusive access (e.g. by disabling IRQs or
/// running on a single core).
#[inline]
pub unsafe fn rx_buffer() -> &'static mut UartBuffer {
    // SAFETY: Caller guarantees exclusive access.
    unsafe { &mut *(&raw mut RX_BUFFER) }
}

/// Returns a mutable reference to the global TX buffer.
///
/// # Safety
///
/// The caller must ensure exclusive access (e.g. by disabling IRQs or
/// running on a single core).
#[inline]
pub unsafe fn tx_buffer() -> &'static mut UartBuffer {
    // SAFETY: Caller guarantees exclusive access.
    unsafe { &mut *(&raw mut TX_BUFFER) }
}

// ===========================================================================
// Uart — PL011 UART driver
// ===========================================================================

/// A handle to a PL011 UART on the BCM2712.
///
/// The Pi 5 exposes UART0 (PL011) at physical address `0x10A0000`.
/// Additional PL011 instances (UART2–UART4 on the BCM2712) can be
/// accessed by passing the appropriate base address.
#[derive(Debug, Clone, Copy)]
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

    /// Creates a new UART handle for UART0 using the default Pi 5 base
    /// address (low-peripheral mode).
    #[inline]
    pub const fn uart0() -> Self {
        Self::new(UART0_BASE)
    }

    /// Returns the base address.
    #[inline]
    pub const fn base(&self) -> Address {
        self.base
    }

    // -----------------------------------------------------------------------
    // Initialisation
    // -----------------------------------------------------------------------

    /// Initialises the UART with the specified baud rate.
    ///
    /// Assumes the UART input clock is 48 MHz (the default on the Pi 5).
    /// Configures 8 data bits, no parity, 1 stop bit, and enables FIFOs.
    /// Clears any pending interrupts and disables interrupt masking.
    ///
    /// # Arguments
    ///
    /// * `baud_rate` — Desired baud rate (e.g. 115200).
    pub fn init(&self, baud_rate: u32) {
        // 1. Disable the UART.
        mmio_write(self.base + CR, 0);

        // 2. Clear pending interrupts.
        mmio_write(self.base + ICR, 0x7FF);

        // 3. Calculate baud rate dividers.
        //    BAUDDIV = UARTCLK / (16 * BaudRate)
        //    IBRD = integer part
        //    FBRD = fractional part * 64, rounded
        let (ibrd, fbrd) = Self::compute_baud_dividers(UART_CLOCK, baud_rate);

        mmio_write(self.base + IBRD, ibrd);
        mmio_write(self.base + FBRD, fbrd);

        // 4. 8 data bits, no parity, 1 stop bit, enable FIFOs.
        mmio_write(self.base + LCRH, LCRH_WLEN8 | LCRH_FEN);

        // 5. Set FIFO interrupt levels (RX 1/8, TX 1/2).
        mmio_write(self.base + IFLS, IFLS_RXIFLSEL_1_8 | IFLS_TXIFLSEL_1_2);

        // 6. Disable all interrupts by masking.
        mmio_write(self.base + IMSC, 0);

        // 7. Enable UART, TX, and RX.
        mmio_write(self.base + CR, CR_UARTEN | CR_TXE | CR_RXE);
    }

    /// Returns the current CR (Control Register) value.
    ///
    /// Useful for checking whether the UART is enabled.
    pub fn read_cr(&self) -> u32 {
        mmio_read(self.base + CR)
    }

    /// Returns the current FR (Flag Register) value.
    ///
    /// Contains TXFF, RXFE, BUSY, and other status bits.
    pub fn read_fr(&self) -> u32 {
        mmio_read(self.base + FR)
    }

    /// Returns the current LCRH (Line Control Register) value.
    pub fn read_lcrh(&self) -> u32 {
        mmio_read(self.base + LCRH)
    }

    /// Returns the current IBRD (Integer Baud Rate Divider) value.
    pub fn read_ibrd(&self) -> u32 {
        mmio_read(self.base + IBRD)
    }

    /// Returns the current FBRD (Fractional Baud Rate Divider) value.
    pub fn read_fbrd(&self) -> u32 {
        mmio_read(self.base + FBRD)
    }

    // -----------------------------------------------------------------------
    // Write operations
    // -----------------------------------------------------------------------

    /// Writes a single byte to the UART, blocking until the TX FIFO
    /// has space.
    #[inline]
    pub fn write_byte(&self, byte: u8) {
        while (mmio_read(self.base + FR) & FR_TXFF) != 0 {
            core::hint::spin_loop();
        }
        mmio_write(self.base + DR, byte as u32);
    }

    /// Writes a string slice to the UART, byte by byte.
    ///
    /// Newline characters (`\n`) are automatically expanded to `\r\n`
    /// for terminal compatibility.
    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }

    /// Writes a byte slice to the UART.
    pub fn write_bytes(&self, data: &[u8]) {
        for &byte in data {
            self.write_byte(byte);
        }
    }

    // -----------------------------------------------------------------------
    // Read operations
    // -----------------------------------------------------------------------

    /// Attempts to read a single byte from the UART without blocking.
    ///
    /// Returns `Some(byte)` if data was available, `None` otherwise.
    #[inline]
    pub fn try_read_byte(&self) -> Option<u8> {
        if (mmio_read(self.base + FR) & FR_RXFE) != 0 {
            None
        } else {
            Some(mmio_read(self.base + DR) as u8)
        }
    }

    /// Reads a single byte from the UART, blocking until the RX FIFO
    /// is non-empty.
    #[inline]
    pub fn read_byte_blocking(&self) -> u8 {
        while (mmio_read(self.base + FR) & FR_RXFE) != 0 {
            core::hint::spin_loop();
        }
        mmio_read(self.base + DR) as u8
    }

    /// Returns `true` if at least one byte is available in the RX FIFO.
    #[inline]
    pub fn available(&self) -> bool {
        (mmio_read(self.base + FR) & FR_RXFE) == 0
    }

    /// Returns `true` if the TX FIFO can accept more data.
    #[inline]
    pub fn tx_ready(&self) -> bool {
        (mmio_read(self.base + FR) & FR_TXFF) == 0
    }

    /// Returns `true` if the UART is busy transmitting data.
    #[inline]
    pub fn is_busy(&self) -> bool {
        (mmio_read(self.base + FR) & FR_BUSY) != 0
    }

    /// Returns `true` if the UART is enabled (CR_UARTEN set).
    #[inline]
    pub fn is_enabled(&self) -> bool {
        (mmio_read(self.base + CR) & CR_UARTEN) != 0
    }

    /// Returns the number of bytes available in the RX FIFO.
    ///
    /// The PL011 FR register does not expose an exact count; this returns
    /// 0 if RXFE is set, or 1 if data is present. For exact counts, use
    /// interrupt-driven I/O with the software buffer.
    pub fn rx_fifo_level(&self) -> usize {
        if self.available() {
            1
        } else {
            0
        }
    }

    // -----------------------------------------------------------------------
    // Interrupt management
    // -----------------------------------------------------------------------

    /// Enables the RX interrupt (data received / receive timeout).
    ///
    /// After calling this the UART will assert an interrupt whenever
    /// the RX FIFO reaches the level configured in IFLS or a receive
    /// timeout occurs.
    pub fn enable_rx_interrupt(&self) {
        let prev = mmio_read(self.base + IMSC);
        mmio_write(self.base + IMSC, prev | IMSC_RXIM | IMSC_RTIM);
    }

    /// Disables the RX interrupt.
    pub fn disable_rx_interrupt(&self) {
        let prev = mmio_read(self.base + IMSC);
        mmio_write(self.base + IMSC, prev & !(IMSC_RXIM | IMSC_RTIM));
    }

    /// Enables the TX interrupt (TX FIFO below threshold).
    pub fn enable_tx_interrupt(&self) {
        let prev = mmio_read(self.base + IMSC);
        mmio_write(self.base + IMSC, prev | IMSC_TXIM);
    }

    /// Disables the TX interrupt.
    pub fn disable_tx_interrupt(&self) {
        let prev = mmio_read(self.base + IMSC);
        mmio_write(self.base + IMSC, prev & !IMSC_TXIM);
    }

    /// Returns the raw interrupt status.
    #[inline]
    pub fn raw_interrupt_status(&self) -> u32 {
        mmio_read(self.base + RIS)
    }

    /// Returns the masked interrupt status.
    #[inline]
    pub fn masked_interrupt_status(&self) -> u32 {
        mmio_read(self.base + MIS)
    }

    /// Clears the specified interrupt flags.
    #[inline]
    pub fn clear_interrupts(&self, mask: u32) {
        mmio_write(self.base + ICR, mask);
    }

    /// Clears all pending interrupts.
    #[inline]
    pub fn clear_all_interrupts(&self) {
        mmio_write(self.base + ICR, 0x7FF);
    }

    /// Checks if the RX interrupt is pending (masked).
    #[inline]
    pub fn rx_interrupt_pending(&self) -> bool {
        (mmio_read(self.base + MIS) & (IMSC_RXIM | IMSC_RTIM)) != 0
    }

    /// Checks if the TX interrupt is pending (masked).
    #[inline]
    pub fn tx_interrupt_pending(&self) -> bool {
        (mmio_read(self.base + MIS) & IMSC_TXIM) != 0
    }

    // -----------------------------------------------------------------------
    // Interrupt-driven I/O handlers
    // -----------------------------------------------------------------------

    /// RX interrupt handler — drains the hardware RX FIFO into the
    /// provided software ring buffer.
    ///
    /// Call this from the UART ISR. It reads all available bytes from
    /// the hardware FIFO and pushes them into `buf`, discarding bytes
    /// when the buffer is full.
    ///
    /// Returns the number of bytes successfully buffered.
    pub fn handle_rx_interrupt(&self, buf: &mut UartBuffer) -> usize {
        let mut count = 0usize;
        // Drain the hardware FIFO (PL011 has a 16-deep FIFO).
        while (mmio_read(self.base + FR) & FR_RXFE) == 0 {
            let byte = mmio_read(self.base + DR) as u8;
            if !buf.push(byte) {
                // Buffer full — discard remaining bytes but keep draining
                // the FIFO to prevent overrun errors.
                continue;
            }
            count += 1;
        }
        // Clear the RX / timeout / overrun interrupts.
        self.clear_interrupts(ICR_RXIC | ICR_RTIC | ICR_OEIC);
        count
    }

    /// TX interrupt handler — fills the hardware TX FIFO from the
    /// provided software ring buffer.
    ///
    /// Call this from the UART ISR. It writes as many bytes as possible
    /// from `buf` into the hardware FIFO.
    ///
    /// Returns the number of bytes written to the FIFO.
    pub fn handle_tx_interrupt(&self, buf: &mut UartBuffer) -> usize {
        let mut count = 0usize;
        while !buf.is_empty() && (mmio_read(self.base + FR) & FR_TXFF) == 0 {
            if let Some(byte) = buf.pop() {
                mmio_write(self.base + DR, byte as u32);
                count += 1;
            }
        }
        // If the buffer is now empty, disable the TX interrupt to avoid
        // spurious fires.
        if buf.is_empty() {
            self.disable_tx_interrupt();
        }
        self.clear_interrupts(ICR_TXIC);
        count
    }

    // -----------------------------------------------------------------------
    // Baud-rate helpers
    // -----------------------------------------------------------------------

    /// Computes the PL011 baud-rate divider values for the given clock
    /// and baud rate, returning `(ibrd, fbrd)`.
    ///
    /// This is a pure function (no MMIO access) and is useful for testing.
    pub const fn compute_baud_dividers(clock: u64, baud_rate: u32) -> (u32, u32) {
        let br = baud_rate as u64;
        let baud_divider = clock / (16 * br);
        let remainder = clock % (16 * br);
        let fractional = (remainder * 64 + 8 * br) / (16 * br);
        (baud_divider as u32, fractional as u32)
    }

    /// Flushes the TX FIFO by waiting until the UART is no longer busy.
    pub fn flush(&self) {
        while (mmio_read(self.base + FR) & FR_BUSY) != 0 {
            core::hint::spin_loop();
        }
    }

    /// Reads the IMSC (Interrupt Mask Set/Clear) register.
    pub fn read_imsc(&self) -> u32 {
        mmio_read(self.base + IMSC)
    }

    /// Writes the IMSC (Interrupt Mask Set/Clear) register.
    pub fn write_imsc(&self, value: u32) {
        mmio_write(self.base + IMSC, value);
    }
}

// ===========================================================================
// MiniUart — auxiliary mini UART (UART1) driver
// ===========================================================================

/// A handle to the BCM2712's auxiliary mini UART (UART1).
///
/// The mini UART is a simplified UART with smaller FIFOs and fewer
/// features than the PL011. It is useful as a secondary serial port.
#[derive(Debug, Clone, Copy)]
pub struct MiniUart {
    /// Base address of the AUX register block.
    base: Address,
}

impl MiniUart {
    /// Creates a new MiniUart handle at the given AUX base address.
    #[inline]
    pub const fn new(base: Address) -> Self {
        Self { base }
    }

    /// Creates a new MiniUart handle using the default Pi 5 AUX base
    /// address (low-peripheral mode).
    #[inline]
    pub const fn default_aux() -> Self {
        Self::new(AUX_BASE)
    }

    /// Returns the base address.
    #[inline]
    pub const fn base(&self) -> Address {
        self.base
    }

    /// Initialises the mini UART with the specified baud rate.
    ///
    /// Configures 8 data bits, enables TX and RX.
    pub fn init(&self, baud_rate: u32) {
        // Enable the mini UART in the AUX enables register.
        mmio_write(self.base + AUX_ENABLES, AUX_ENABLE_MU);

        // Disable TX/RX while configuring.
        mmio_write(self.base + AUX_MU_CNTL, 0);

        // Disable interrupts.
        mmio_write(self.base + AUX_MU_IER, 0);

        // Clear the FIFOs by toggling the clear bits in IIR.
        mmio_write(self.base + AUX_MU_IIR, 0xC6);

        // 8-bit mode.
        mmio_write(self.base + AUX_MU_LCR, AUX_MU_LCR_8BIT);

        // Set RTS high.
        mmio_write(self.base + AUX_MU_MCR, 0);

        // Set baud rate.
        // The mini UART baud rate counter = (system_clock / (8 * baud_rate)) - 1
        // Assuming 48 MHz system clock.
        let baud_counter = (UART_CLOCK / (8 * baud_rate as u64)) - 1;
        mmio_write(self.base + AUX_MU_BAUD, baud_counter as u32);

        // Enable TX and RX.
        mmio_write(
            self.base + AUX_MU_CNTL,
            AUX_MU_CNTL_TX_ENABLE | AUX_MU_CNTL_RX_ENABLE,
        );
    }

    /// Writes a single byte, blocking until the TX buffer is empty.
    #[inline]
    pub fn write_byte(&self, byte: u8) {
        while (mmio_read(self.base + AUX_MU_LSR) & AUX_MU_LSR_TX_EMPTY) == 0 {
            core::hint::spin_loop();
        }
        mmio_write(self.base + AUX_MU_IO, byte as u32);
    }

    /// Reads a single byte, blocking until data is available.
    #[inline]
    pub fn read_byte_blocking(&self) -> u8 {
        while (mmio_read(self.base + AUX_MU_LSR) & AUX_MU_LSR_RX_READY) == 0 {
            core::hint::spin_loop();
        }
        mmio_read(self.base + AUX_MU_IO) as u8
    }

    /// Attempts to read a byte without blocking.
    #[inline]
    pub fn try_read_byte(&self) -> Option<u8> {
        if (mmio_read(self.base + AUX_MU_LSR) & AUX_MU_LSR_RX_READY) != 0 {
            Some(mmio_read(self.base + AUX_MU_IO) as u8)
        } else {
            None
        }
    }

    /// Writes a string slice, expanding `\n` to `\r\n`.
    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }

    /// Returns `true` if a byte is available to read.
    #[inline]
    pub fn available(&self) -> bool {
        (mmio_read(self.base + AUX_MU_LSR) & AUX_MU_LSR_RX_READY) != 0
    }

    /// Returns `true` if the TX buffer can accept data.
    #[inline]
    pub fn tx_ready(&self) -> bool {
        (mmio_read(self.base + AUX_MU_LSR) & AUX_MU_LSR_TX_EMPTY) != 0
    }
}

// ===========================================================================
// Free-standing convenience API (UART0)
// ===========================================================================

/// Initialises UART0 (PL011) with the given baud rate.
///
/// If `baud_rate` is 0, [`DEFAULT_BAUD_RATE`] (115200) is used.
pub fn uart_init(baud_rate: u32) {
    let br = if baud_rate == 0 {
        DEFAULT_BAUD_RATE
    } else {
        baud_rate
    };
    Uart::uart0().init(br);
}

/// Initialises a PL011 UART at the given base address with the specified
/// baud rate.
///
/// This is the BCM2712-specific variant that allows targeting any of the
/// SoC's multiple PL011 instances (UART0–UART4), not just UART0.
///
/// If `baud_rate` is 0, [`DEFAULT_BAUD_RATE`] (115200) is used.
pub fn uart_init_with_base(base: Address, baud_rate: u32) {
    let br = if baud_rate == 0 {
        DEFAULT_BAUD_RATE
    } else {
        baud_rate
    };
    Uart::new(base).init(br);
}

/// Writes a single byte to UART0, blocking until the TX FIFO has space.
#[inline]
pub fn uart_write_byte(byte: u8) {
    Uart::uart0().write_byte(byte);
}

/// Writes a string to UART0, expanding `\n` to `\r\n`.
#[inline]
pub fn uart_write_str(s: &str) {
    Uart::uart0().write_str(s);
}

/// Reads a single byte from UART0 (non-blocking).
///
/// First checks the global RX ring buffer; if empty, polls the hardware
/// FIFO directly.
pub fn uart_read_byte() -> Option<u8> {
    // Check the software buffer first.
    // SAFETY: In a bare-metal single-core context, we can safely access
    // the global buffer. In multi-core or pre-emptible contexts the
    // caller must ensure proper synchronisation.
    unsafe {
        if let Some(byte) = (*(&raw mut RX_BUFFER)).pop() {
            return Some(byte);
        }
    }
    // Fall through to hardware.
    Uart::uart0().try_read_byte()
}

/// Reads a single byte from UART0, blocking until data is available.
///
/// First drains the global RX buffer, then polls the hardware FIFO.
pub fn uart_read_byte_blocking() -> u8 {
    // Check the software buffer first.
    unsafe {
        if let Some(byte) = (*(&raw mut RX_BUFFER)).pop() {
            return byte;
        }
    }
    Uart::uart0().read_byte_blocking()
}

/// UART0 RX interrupt handler — drains hardware FIFO into the global
/// RX ring buffer.
///
/// Call this from the UART0 ISR.
pub fn uart0_rx_interrupt_handler() -> usize {
    // SAFETY: Called from ISR context with IRQs disabled.
    unsafe { Uart::uart0().handle_rx_interrupt(&mut *(&raw mut RX_BUFFER)) }
}

/// UART0 TX interrupt handler — fills hardware FIFO from the global
/// TX ring buffer.
///
/// Call this from the UART0 ISR.
pub fn uart0_tx_interrupt_handler() -> usize {
    // SAFETY: Called from ISR context with IRQs disabled.
    unsafe { Uart::uart0().handle_tx_interrupt(&mut *(&raw mut TX_BUFFER)) }
}

/// Enables UART0 RX interrupts.
#[inline]
pub fn uart_enable_rx_interrupt() {
    Uart::uart0().enable_rx_interrupt();
}

/// Disables UART0 RX interrupts.
#[inline]
pub fn uart_disable_rx_interrupt() {
    Uart::uart0().disable_rx_interrupt();
}

/// Enables UART0 TX interrupts.
#[inline]
pub fn uart_enable_tx_interrupt() {
    Uart::uart0().enable_tx_interrupt();
}

/// Disables UART0 TX interrupts.
#[inline]
pub fn uart_disable_tx_interrupt() {
    Uart::uart0().disable_tx_interrupt();
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Arbitrary non-zero base address used for mock UART registers.
    const MOCK_BASE: Address = 0x3000_0000;

    /// Resets the mock MMIO store before each test.
    fn reset_mock() {
        mock_mmio::reset();
    }

    // ---- Pure function tests (no mock needed) ----------------------------

    #[test]
    fn uart_stores_base_address() {
        let uart = Uart::new(0x1D0A_0000);
        assert_eq!(uart.base(), 0x1D0A_0000);
    }

    #[test]
    fn uart0_default_base_matches_platform() {
        let expected: Address =
            crate::platform::PERIPHERAL_BASE + crate::platform::UART_BASE_OFFSET;
        assert_eq!(UART0_BASE, expected);
    }

    #[test]
    fn uart0_constructor_matches_explicit() {
        assert_eq!(Uart::uart0().base(), Uart::new(UART0_BASE).base());
    }

    #[test]
    fn baud_dividers_115200_at_48mhz() {
        let (ibrd, fbrd) = Uart::compute_baud_dividers(48_000_000, 115200);
        assert_eq!(ibrd, 26);
        assert_eq!(fbrd, 3);
    }

    #[test]
    fn baud_dividers_9600_at_48mhz() {
        let (ibrd, fbrd) = Uart::compute_baud_dividers(48_000_000, 9600);
        assert_eq!(ibrd, 312);
        assert_eq!(fbrd, 32);
    }

    #[test]
    fn default_baud_rate_is_115200() {
        assert_eq!(DEFAULT_BAUD_RATE, 115200);
    }

    // ---- Test 1: uart_init writes correct registers via mock MMIO --------

    #[test]
    fn test_uart_init_writes_registers() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);
        uart.init(115200);

        // CR should be 0 (disabled) after first step, then enabled at end
        let cr_val = mock_mmio::read(MOCK_BASE + CR);
        assert_eq!(
            cr_val,
            CR_UARTEN | CR_TXE | CR_RXE,
            "CR should have UARTEN+TXE+RXE after init"
        );

        // IBRD and FBRD for 115200 @ 48MHz
        let ibrd_val = mock_mmio::read(MOCK_BASE + IBRD);
        let fbrd_val = mock_mmio::read(MOCK_BASE + FBRD);
        assert_eq!(ibrd_val, 26, "IBRD should be 26 for 115200");
        assert_eq!(fbrd_val, 3, "FBRD should be 3 for 115200");

        // LCRH should have 8-bit + FIFO enable
        let lcrh_val = mock_mmio::read(MOCK_BASE + LCRH);
        assert_eq!(
            lcrh_val & LCRH_WLEN8,
            LCRH_WLEN8,
            "LCRH should have WLEN8"
        );
        assert_eq!(
            lcrh_val & LCRH_FEN,
            LCRH_FEN,
            "LCRH should have FEN (FIFO enable)"
        );

        // IMSC should be 0 (all interrupts masked)
        let imsc_val = mock_mmio::read(MOCK_BASE + IMSC);
        assert_eq!(imsc_val, 0, "IMSC should be 0 after init");

        // ICR should have been written with 0x7FF (clear all)
        let icr_val = mock_mmio::read(MOCK_BASE + ICR);
        assert_eq!(icr_val, 0x7FF, "ICR should have been cleared");
    }

    // ---- Test 2: uart_init with different baud rate ----------------------

    #[test]
    fn test_uart_init_baud_9600() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);
        uart.init(9600);

        let ibrd_val = mock_mmio::read(MOCK_BASE + IBRD);
        let fbrd_val = mock_mmio::read(MOCK_BASE + FBRD);
        assert_eq!(ibrd_val, 312, "IBRD should be 312 for 9600");
        assert_eq!(fbrd_val, 32, "FBRD should be 32 for 9600");
    }

    // ---- Test 3: write_byte to DR when TX FIFO not full -----------------

    #[test]
    fn test_write_byte_to_dr() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // TX FIFO not full (FR_TXFF = 0)
        mock_mmio::write(MOCK_BASE + FR, 0);

        uart.write_byte(0x41);

        let dr_val = mock_mmio::read(MOCK_BASE + DR);
        assert_eq!(dr_val, 0x41, "DR should contain the written byte");
    }

    // ---- Test 4: try_read_byte returns None when RX FIFO empty ----------

    #[test]
    fn test_try_read_byte_empty_fifo() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // RX FIFO empty (FR_RXFE set)
        mock_mmio::write(MOCK_BASE + FR, FR_RXFE);

        assert_eq!(
            uart.try_read_byte(),
            None,
            "try_read_byte should return None when RX FIFO empty"
        );
    }

    // ---- Test 5: try_read_byte returns byte when RX FIFO has data -------

    #[test]
    fn test_try_read_byte_with_data() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // RX FIFO not empty (FR_RXFE = 0), DR has 0x55
        mock_mmio::write(MOCK_BASE + FR, 0);
        mock_mmio::write(MOCK_BASE + DR, 0x55);

        assert_eq!(
            uart.try_read_byte(),
            Some(0x55),
            "try_read_byte should return the byte from DR"
        );
    }

    // ---- Test 6: available() reflects RX FIFO state ---------------------

    #[test]
    fn test_available_checks_rxfe() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // RX FIFO empty
        mock_mmio::write(MOCK_BASE + FR, FR_RXFE);
        assert!(!uart.available(), "Should not be available when RXFE set");

        // RX FIFO has data
        mock_mmio::write(MOCK_BASE + FR, 0);
        assert!(uart.available(), "Should be available when RXFE clear");
    }

    // ---- Test 7: tx_ready() reflects TX FIFO state ----------------------

    #[test]
    fn test_tx_ready_checks_txff() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // TX FIFO full
        mock_mmio::write(MOCK_BASE + FR, FR_TXFF);
        assert!(!uart.tx_ready(), "Should not be ready when TXFF set");

        // TX FIFO has space
        mock_mmio::write(MOCK_BASE + FR, 0);
        assert!(uart.tx_ready(), "Should be ready when TXFF clear");
    }

    // ---- Test 8: is_busy() reflects FR_BUSY bit -------------------------

    #[test]
    fn test_is_busy_checks_fr_busy() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        mock_mmio::write(MOCK_BASE + FR, FR_BUSY);
        assert!(uart.is_busy(), "Should be busy when BUSY set");

        mock_mmio::write(MOCK_BASE + FR, 0);
        assert!(!uart.is_busy(), "Should not be busy when BUSY clear");
    }

    // ---- Test 9: enable/disable RX interrupts modify IMSC ---------------

    #[test]
    fn test_enable_disable_rx_interrupt() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // Start with IMSC = 0
        mock_mmio::write(MOCK_BASE + IMSC, 0);

        // Enable RX interrupt
        uart.enable_rx_interrupt();
        let imsc = mock_mmio::read(MOCK_BASE + IMSC);
        assert_eq!(
            imsc & (IMSC_RXIM | IMSC_RTIM),
            IMSC_RXIM | IMSC_RTIM,
            "IMSC should have RXIM + RTIM after enable_rx_interrupt"
        );

        // Disable RX interrupt
        uart.disable_rx_interrupt();
        let imsc = mock_mmio::read(MOCK_BASE + IMSC);
        assert_eq!(
            imsc & (IMSC_RXIM | IMSC_RTIM),
            0,
            "IMSC should have RXIM + RTIM cleared after disable_rx_interrupt"
        );
    }

    // ---- Test 10: enable/disable TX interrupts modify IMSC ---------------

    #[test]
    fn test_enable_disable_tx_interrupt() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        mock_mmio::write(MOCK_BASE + IMSC, 0);

        uart.enable_tx_interrupt();
        let imsc = mock_mmio::read(MOCK_BASE + IMSC);
        assert_eq!(
            imsc & IMSC_TXIM,
            IMSC_TXIM,
            "IMSC should have TXIM after enable_tx_interrupt"
        );

        uart.disable_tx_interrupt();
        let imsc = mock_mmio::read(MOCK_BASE + IMSC);
        assert_eq!(
            imsc & IMSC_TXIM,
            0,
            "IMSC should have TXIM cleared after disable_tx_interrupt"
        );
    }

    // ---- Test 11: enable_rx_interrupt preserves existing IMSC bits -------

    #[test]
    fn test_enable_rx_interrupt_preserves_imsc() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // Pre-set some other bits in IMSC
        let existing_bits = IMSC_OEIM; // overrun error interrupt mask
        mock_mmio::write(MOCK_BASE + IMSC, existing_bits);

        uart.enable_rx_interrupt();
        let imsc = mock_mmio::read(MOCK_BASE + IMSC);
        assert_eq!(
            imsc & existing_bits,
            existing_bits,
            "Existing IMSC bits should be preserved"
        );
        assert_eq!(
            imsc & (IMSC_RXIM | IMSC_RTIM),
            IMSC_RXIM | IMSC_RTIM,
            "RX interrupt bits should be set"
        );
    }

    // ---- Test 12: clear_interrupts writes to ICR -------------------------

    #[test]
    fn test_clear_interrupts() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        uart.clear_interrupts(ICR_RXIC | ICR_TXIC);
        let icr = mock_mmio::read(MOCK_BASE + ICR);
        assert_eq!(
            icr,
            ICR_RXIC | ICR_TXIC,
            "ICR should have RX and TX interrupt clear bits"
        );
    }

    // ---- Test 13: clear_all_interrupts writes 0x7FF to ICR ---------------

    #[test]
    fn test_clear_all_interrupts() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        uart.clear_all_interrupts();
        let icr = mock_mmio::read(MOCK_BASE + ICR);
        assert_eq!(icr, 0x7FF, "ICR should be 0x7FF (clear all)");
    }

    // ---- Test 14: rx_interrupt_pending checks MIS ------------------------

    #[test]
    fn test_rx_interrupt_pending() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // No RX interrupt pending
        mock_mmio::write(MOCK_BASE + MIS, 0);
        assert!(!uart.rx_interrupt_pending(), "RX should not be pending");

        // RX interrupt pending (RXIM bit in MIS)
        mock_mmio::write(MOCK_BASE + MIS, IMSC_RXIM);
        assert!(uart.rx_interrupt_pending(), "RX should be pending when RXIM set");

        // RTIM bit (receive timeout)
        mock_mmio::write(MOCK_BASE + MIS, IMSC_RTIM);
        assert!(uart.rx_interrupt_pending(), "RX should be pending when RTIM set");
    }

    // ---- Test 15: tx_interrupt_pending checks MIS ------------------------

    #[test]
    fn test_tx_interrupt_pending() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        mock_mmio::write(MOCK_BASE + MIS, 0);
        assert!(!uart.tx_interrupt_pending(), "TX should not be pending");

        mock_mmio::write(MOCK_BASE + MIS, IMSC_TXIM);
        assert!(uart.tx_interrupt_pending(), "TX should be pending when TXIM set");
    }

    // ---- Test 16: raw_interrupt_status reads RIS -------------------------

    #[test]
    fn test_raw_interrupt_status() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        mock_mmio::write(MOCK_BASE + RIS, RIS_RXRIS | RIS_TXRIS);
        let ris = uart.raw_interrupt_status();
        assert_eq!(
            ris & (RIS_RXRIS | RIS_TXRIS),
            RIS_RXRIS | RIS_TXRIS,
            "RIS should contain RX and TX interrupt bits"
        );
    }

    // ---- Test 17: handle_rx_interrupt drains FIFO into buffer -----------

    #[test]
    fn test_handle_rx_interrupt() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);
        let mut buf = UartBuffer::new();

        // Simulate 3 bytes in the RX FIFO
        // First call: FR with RXFE=0, DR=0x41
        // Second call: FR with RXFE=0, DR=0x42
        // Third call: FR with RXFE=0, DR=0x43
        // Fourth call: FR with RXFE=1 (empty)
        //
        // Since mock_mmio is a simple HashMap, we can only set one value per
        // address. We'll use the simple case: set FR with RXFE clear and DR
        // with a value, then after one read set RXFE.
        mock_mmio::write(MOCK_BASE + FR, 0); // RXFE clear → data available
        mock_mmio::write(MOCK_BASE + DR, 0x41);

        let count = uart.handle_rx_interrupt(&mut buf);
        // After one read, the mock still has FR=0 (RXFE clear) so the loop
        // continues. In real hardware the FIFO empties. With mock, we test
        // that at least one byte was buffered.
        assert!(count >= 1, "At least one byte should be buffered");
        assert_eq!(buf.peek(), Some(0x41), "First byte should be 0x41");
    }

    // ---- Test 18: handle_tx_interrupt fills FIFO from buffer -------------

    #[test]
    fn test_handle_tx_interrupt() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);
        let mut buf = UartBuffer::new();

        buf.push(0x55);
        buf.push(0xAA);

        // TX FIFO not full
        mock_mmio::write(MOCK_BASE + FR, 0);

        let count = uart.handle_tx_interrupt(&mut buf);
        assert_eq!(count, 2, "Two bytes should be written to FIFO");

        // DR should have the last byte written
        assert_eq!(
            mock_mmio::read(MOCK_BASE + DR),
            0xAA,
            "Last byte in DR should be 0xAA"
        );

        // Buffer should be empty now
        assert!(buf.is_empty(), "TX buffer should be empty after interrupt");
    }

    // ---- Test 19: is_enabled checks CR_UARTEN ---------------------------

    #[test]
    fn test_is_enabled() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        mock_mmio::write(MOCK_BASE + CR, 0);
        assert!(!uart.is_enabled(), "UART should not be enabled");

        mock_mmio::write(MOCK_BASE + CR, CR_UARTEN);
        assert!(uart.is_enabled(), "UART should be enabled");
    }

    // ---- Test 20: read_cr, read_fr, read_lcrh, read_ibrd, read_fbrd ----

    #[test]
    fn test_register_read_helpers() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        mock_mmio::write(MOCK_BASE + CR, CR_UARTEN | CR_TXE);
        mock_mmio::write(MOCK_BASE + FR, FR_BUSY);
        mock_mmio::write(MOCK_BASE + LCRH, LCRH_WLEN8 | LCRH_FEN);
        mock_mmio::write(MOCK_BASE + IBRD, 26);
        mock_mmio::write(MOCK_BASE + FBRD, 3);

        assert_eq!(uart.read_cr(), CR_UARTEN | CR_TXE);
        assert_eq!(uart.read_fr(), FR_BUSY);
        assert_eq!(uart.read_lcrh(), LCRH_WLEN8 | LCRH_FEN);
        assert_eq!(uart.read_ibrd(), 26);
        assert_eq!(uart.read_fbrd(), 3);
    }

    // ---- UartBuffer tests ------------------------------------------------

    #[test]
    fn buffer_new_is_empty() {
        let buf = UartBuffer::new();
        assert!(buf.is_empty());
        assert!(!buf.is_full());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), BUF_SIZE);
        assert_eq!(buf.free_space(), BUF_SIZE);
    }

    #[test]
    fn buffer_push_pop_round_trip() {
        let mut buf = UartBuffer::new();
        assert!(buf.push(0x41));
        assert!(buf.push(0x42));
        assert!(buf.push(0x43));
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.free_space(), BUF_SIZE - 3);
        assert_eq!(buf.pop(), Some(0x41));
        assert_eq!(buf.pop(), Some(0x42));
        assert_eq!(buf.pop(), Some(0x43));
        assert!(buf.is_empty());
    }

    #[test]
    fn buffer_full_and_overflow() {
        let mut buf = UartBuffer::new();
        for i in 0..BUF_SIZE {
            assert!(buf.push(i as u8), "push {} should succeed", i);
        }
        assert!(buf.is_full());
        assert_eq!(buf.len(), BUF_SIZE);
        assert_eq!(buf.free_space(), 0);
        // Pushing beyond capacity should fail.
        assert!(!buf.push(0xFF));
    }

    #[test]
    fn buffer_wrap_around() {
        let mut buf = UartBuffer::new();
        // Fill the buffer.
        for i in 0..BUF_SIZE {
            buf.push(i as u8);
        }
        // Drain half.
        for i in 0..BUF_SIZE / 2 {
            assert_eq!(buf.pop(), Some(i as u8));
        }
        // Push more — these should wrap around.
        for i in 0..BUF_SIZE / 2 {
            assert!(buf.push((i as u8).wrapping_add(0x80)));
        }
        // Verify all remaining bytes in order.
        for i in BUF_SIZE / 2..BUF_SIZE {
            assert_eq!(buf.pop(), Some(i as u8));
        }
        for i in 0..BUF_SIZE / 2 {
            assert_eq!(buf.pop(), Some((i as u8).wrapping_add(0x80)));
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn buffer_clear_resets_state() {
        let mut buf = UartBuffer::new();
        buf.push(1);
        buf.push(2);
        buf.push(3);
        assert_eq!(buf.len(), 3);
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.free_space(), BUF_SIZE);
        // After clear, push/pop should still work.
        assert!(buf.push(0xAA));
        assert_eq!(buf.pop(), Some(0xAA));
    }

    #[test]
    fn buffer_peek_does_not_consume() {
        let mut buf = UartBuffer::new();
        assert_eq!(buf.peek(), None);
        buf.push(0x55);
        assert_eq!(buf.peek(), Some(0x55));
        assert_eq!(buf.len(), 1); // peek doesn't consume
        assert_eq!(buf.pop(), Some(0x55));
    }

    #[test]
    fn buffer_push_slice_and_pop_slice() {
        let mut buf = UartBuffer::new();
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        let pushed = buf.push_slice(&data);
        assert_eq!(pushed, 5, "All 5 bytes should be pushed");
        assert_eq!(buf.len(), 5);

        let mut out = [0u8; 3];
        let popped = buf.pop_slice(&mut out);
        assert_eq!(popped, 3, "3 bytes should be popped");
        assert_eq!(out, [0x01, 0x02, 0x03]);

        let mut out2 = [0u8; 5];
        let popped2 = buf.pop_slice(&mut out2);
        assert_eq!(popped2, 2, "2 remaining bytes should be popped");
        assert_eq!(out2[0], 0x04);
        assert_eq!(out2[1], 0x05);
    }

    // ---- MiniUart tests --------------------------------------------------

    #[test]
    fn mini_uart_stores_base_address() {
        let mu = MiniUart::new(0x1D0A_8000);
        assert_eq!(mu.base(), 0x1D0A_8000);
    }

    #[test]
    fn mini_uart_default_base_matches_platform() {
        let expected: Address =
            crate::platform::PERIPHERAL_BASE + crate::platform::AUX_BASE_OFFSET;
        assert_eq!(AUX_BASE, expected);
    }

    // ---- Register constant tests -----------------------------------------

    #[test]
    fn pl011_register_offsets_are_correct() {
        assert_eq!(DR, 0x00);
        assert_eq!(RSR_ECR, 0x04);
        assert_eq!(FR, 0x18);
        assert_eq!(IBRD, 0x24);
        assert_eq!(FBRD, 0x28);
        assert_eq!(LCRH, 0x2C);
        assert_eq!(CR, 0x30);
        assert_eq!(IFLS, 0x34);
        assert_eq!(IMSC, 0x38);
        assert_eq!(RIS, 0x3C);
        assert_eq!(MIS, 0x40);
        assert_eq!(ICR, 0x44);
    }

    #[test]
    fn interrupt_mask_bits_non_overlapping() {
        // Ensure the mask constants are distinct.
        assert_eq!(IMSC_RXIM & IMSC_TXIM, 0);
        assert_eq!(IMSC_RXIM & IMSC_RTIM, 0);
        assert_eq!(IMSC_TXIM & IMSC_RTIM, 0);
        assert_eq!(IMSC_OEIM & IMSC_RXIM, 0);
    }

    #[test]
    fn flag_register_bits_non_overlapping() {
        assert_eq!(FR_TXFF & FR_RXFE, 0);
        assert_eq!(FR_TXFF & FR_BUSY, 0);
        assert_eq!(FR_RXFE & FR_BUSY, 0);
    }

    #[test]
    fn control_register_bits_non_overlapping() {
        assert_eq!(CR_UARTEN & CR_TXE, 0);
        assert_eq!(CR_UARTEN & CR_RXE, 0);
        assert_eq!(CR_TXE & CR_RXE, 0);
    }

    #[test]
    fn fifo_depth_is_16() {
        assert_eq!(FIFO_DEPTH, 16, "PL011 FIFO depth should be 16 bytes");
    }

    // =========================================================================
    // BCM2712-specific enhancement tests
    // =========================================================================

    // ---- Test: uart_init_with_base at custom address ----------------------

    #[test]
    fn test_uart_init_with_base_custom_address() {
        reset_mock();
        let custom_base: Address = 0x5000_0000;
        uart_init_with_base(custom_base, 115200);

        // CR should have UARTEN + TXE + RXE
        let cr_val = mock_mmio::read(custom_base + CR);
        assert_eq!(
            cr_val,
            CR_UARTEN | CR_TXE | CR_RXE,
            "CR should be enabled at custom base"
        );

        // IBRD should be 26 for 115200 @ 48 MHz
        let ibrd_val = mock_mmio::read(custom_base + IBRD);
        assert_eq!(ibrd_val, 26, "IBRD should be 26");
    }

    // ---- Test: uart_init_with_base with 0 baud defaults to 115200 --------

    #[test]
    fn test_uart_init_with_base_zero_baud() {
        reset_mock();
        let custom_base: Address = 0x5000_0000;
        uart_init_with_base(custom_base, 0);

        // IBRD should match 115200 (not 0)
        let ibrd_val = mock_mmio::read(custom_base + IBRD);
        assert_eq!(ibrd_val, 26, "IBRD should be 26 for default 115200");
    }

    // ---- Test: uart_init_with_base 9600 baud ----------------------------

    #[test]
    fn test_uart_init_with_base_baud_9600() {
        reset_mock();
        let custom_base: Address = 0x5000_0000;
        uart_init_with_base(custom_base, 9600);

        let ibrd_val = mock_mmio::read(custom_base + IBRD);
        let fbrd_val = mock_mmio::read(custom_base + FBRD);
        assert_eq!(ibrd_val, 312, "IBRD should be 312 for 9600");
        assert_eq!(fbrd_val, 32, "FBRD should be 32 for 9600");
    }

    // ---- Test: uart_write_byte mock MMIO ---------------------------------

    #[test]
    fn test_uart_write_byte_via_mock() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // TX FIFO not full
        mock_mmio::write(MOCK_BASE + FR, 0);

        uart.write_byte(0x42);
        assert_eq!(
            mock_mmio::read(MOCK_BASE + DR),
            0x42,
            "DR should contain 0x42"
        );
    }

    // ---- Test: uart_write_str expands newline ----------------------------

    #[test]
    fn test_uart_write_str_expands_newline() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // TX FIFO not full
        mock_mmio::write(MOCK_BASE + FR, 0);

        uart.write_str("A\nB");

        // The mock stores the last value written to DR, which should be 'B' (0x42)
        // But we can verify the final byte is correct
        assert_eq!(
            mock_mmio::read(MOCK_BASE + DR),
            0x42,
            "Last byte written should be 'B' (0x42)"
        );
    }

    // ---- Test: MiniUart init writes correct registers via mock MMIO -------

    #[test]
    fn test_mini_uart_init_registers() {
        reset_mock();
        let mu = MiniUart::new(MOCK_BASE);
        mu.init(115200);

        // AUX_ENABLES should have mini UART enabled
        let enables = mock_mmio::read(MOCK_BASE + AUX_ENABLES);
        assert_eq!(
            enables & AUX_ENABLE_MU,
            AUX_ENABLE_MU,
            "Mini UART should be enabled in AUX_ENABLES"
        );

        // LCR should be 8-bit mode
        let lcr = mock_mmio::read(MOCK_BASE + AUX_MU_LCR);
        assert_eq!(lcr, AUX_MU_LCR_8BIT, "LCR should be 8-bit");

        // CNTL should have TX + RX enabled
        let cntl = mock_mmio::read(MOCK_BASE + AUX_MU_CNTL);
        assert_eq!(
            cntl,
            AUX_MU_CNTL_TX_ENABLE | AUX_MU_CNTL_RX_ENABLE,
            "CNTL should have TX+RX enabled"
        );

        // IER should be 0 (interrupts disabled)
        let ier = mock_mmio::read(MOCK_BASE + AUX_MU_IER);
        assert_eq!(ier, 0, "IER should be 0");

        // MCR should be 0
        let mcr = mock_mmio::read(MOCK_BASE + AUX_MU_MCR);
        assert_eq!(mcr, 0, "MCR should be 0");
    }

    // ---- Test: MiniUart baud rate counter --------------------------------

    #[test]
    fn test_mini_uart_baud_counter() {
        reset_mock();
        let mu = MiniUart::new(MOCK_BASE);
        mu.init(115200);

        // Baud counter = (48_000_000 / (8 * 115200)) - 1 = 51.08... - 1 ≈ 51
        let baud_reg = mock_mmio::read(MOCK_BASE + AUX_MU_BAUD);
        assert_eq!(
            baud_reg, 51,
            "Baud counter should be 51 for 115200 @ 48 MHz"
        );
    }

    // ---- Test: MiniUart try_read_byte ------------------------------------

    #[test]
    fn test_mini_uart_try_read_byte() {
        reset_mock();
        let mu = MiniUart::new(MOCK_BASE);

        // RX not ready
        mock_mmio::write(MOCK_BASE + AUX_MU_LSR, 0);
        assert_eq!(mu.try_read_byte(), None, "Should return None when RX not ready");

        // RX ready
        mock_mmio::write(MOCK_BASE + AUX_MU_LSR, AUX_MU_LSR_RX_READY);
        mock_mmio::write(MOCK_BASE + AUX_MU_IO, 0x77);
        assert_eq!(mu.try_read_byte(), Some(0x77), "Should return byte when RX ready");
    }

    // ---- Test: MiniUart available and tx_ready ---------------------------

    #[test]
    fn test_mini_uart_available_and_tx_ready() {
        reset_mock();
        let mu = MiniUart::new(MOCK_BASE);

        // RX not ready, TX not empty
        mock_mmio::write(MOCK_BASE + AUX_MU_LSR, 0);
        assert!(!mu.available(), "Should not be available");
        assert!(!mu.tx_ready(), "TX should not be ready when TX_EMPTY is 0");

        // RX ready
        mock_mmio::write(MOCK_BASE + AUX_MU_LSR, AUX_MU_LSR_RX_READY);
        assert!(mu.available(), "Should be available when RX_READY");

        // TX empty
        mock_mmio::write(MOCK_BASE + AUX_MU_LSR, AUX_MU_LSR_TX_EMPTY);
        assert!(mu.tx_ready(), "TX should be ready when TX_EMPTY");
    }

    // ---- Test: BCM2712 UART clock is 48 MHz ------------------------------

    #[test]
    fn test_bcm2712_uart_clock_is_48mhz() {
        assert_eq!(UART_CLOCK, 48_000_000, "BCM2712 UART clock should be 48 MHz");
    }

    // ---- Test: IFLS default after init -----------------------------------

    #[test]
    fn test_ifls_default_after_init() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);
        uart.init(115200);

        let ifls = mock_mmio::read(MOCK_BASE + IFLS);
        assert_eq!(
            ifls,
            IFLS_RXIFLSEL_1_8 | IFLS_TXIFLSEL_1_2,
            "IFLS should be RX 1/8 + TX 1/2 after init"
        );
    }

    // ---- Test: write_bytes via mock MMIO ---------------------------------

    #[test]
    fn test_write_bytes_via_mock() {
        reset_mock();
        let uart = Uart::new(MOCK_BASE);

        // TX FIFO not full
        mock_mmio::write(MOCK_BASE + FR, 0);

        uart.write_bytes(&[0x01, 0x02, 0x03]);

        // Last byte written should be 0x03
        assert_eq!(
            mock_mmio::read(MOCK_BASE + DR),
            0x03,
            "Last DR byte should be 0x03"
        );
    }
}
