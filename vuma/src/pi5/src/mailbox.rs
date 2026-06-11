//! BCM2712 VideoCore mailbox interface for the Raspberry Pi 5.
//!
//! The VideoCore mailbox provides a communication channel between the ARM
//! CPU and the VideoCore GPU firmware. It is used for querying hardware
//! information (memory layout, board serial, firmware version, etc.) and
//! requesting services (power control, clock management, framebuffer
//! allocation).
//!
//! # Register layout
//!
//! The BCM2712 mailbox registers are memory-mapped at physical base
//! `0x1C00_0000`:
//!
//! | Offset  | Register          | Direction | Description           |
//! |---------|-------------------|-----------|-----------------------|
//! | `0x00`  | MAILBOX0_READ     | ARM ← VC | Read incoming mail     |
//! | `0x18`  | MAILBOX0_STATUS   | R/O       | Status of read FIFO   |
//! | `0x20`  | MAILBOX1_WRITE    | ARM → VC | Write outgoing mail    |
//! | `0x38`  | MAILBOX1_STATUS   | R/O       | Status of write FIFO  |
//!
//! Each 28-bit payload is combined with a 4-bit channel number in the
//! low nibble. Channel 8 is the **ARM-to-VC property message** channel.
//!
//! # Property messages
//!
//! Property messages use a tag-based protocol. The message buffer is a
//! contiguous array of `u32` words laid out as:
//!
//! ```text
//! [0]  total buffer size (bytes, including this word)
//! [1]  request code (0x0000_0000)
//! [2]  tag #1 identity
//! [3]  tag #1 value-buffer size (bytes)
//! [4]  tag #1 request/response code (bit 31 = response flag)
//! [5…] tag #1 value buffer
//! …    additional tags …
//! [N]  end tag (0x0000_0000)
//! ```
//!
//! On return the firmware overwrites the buffer in-place with the
//! response.

use crate::mmio::Address;
// core::ptr is used by aarch64 register access functions.

// ---------------------------------------------------------------------------
// Mailbox register offsets (from MAILBOX_BASE = 0x1C00_0000)
// ---------------------------------------------------------------------------

/// Base physical address of the BCM2712 mailbox registers.
pub const MAILBOX_BASE: Address = 0x1C00_0000;

/// Offset of MAILBOX0_READ from the base — ARM reads incoming mail here.
pub const MAILBOX0_READ: Address = 0x00;

/// Offset of MAILBOX0_STATUS from the base — status of the read FIFO.
pub const MAILBOX0_STATUS: Address = 0x18;

/// Offset of MAILBOX1_WRITE from the base — ARM writes outgoing mail here.
pub const MAILBOX1_WRITE: Address = 0x20;

/// Offset of MAILBOX1_STATUS from the base — status of the write FIFO.
pub const MAILBOX1_STATUS: Address = 0x38;

// ---------------------------------------------------------------------------
// Status register flags
// ---------------------------------------------------------------------------

/// Mailbox FIFO empty flag.
pub const STATUS_EMPTY: u32 = 0x4000_0000;

/// Mailbox FIFO full flag.
pub const STATUS_FULL: u32 = 0x8000_0000;

// ---------------------------------------------------------------------------
// Channel identifiers
// ---------------------------------------------------------------------------

/// Property message channel (ARM → VC, the primary channel for tagged
/// property messages).
pub const CHANNEL_PROPERTY: u32 = 8;

// ---------------------------------------------------------------------------
// Property tag identifiers
// ---------------------------------------------------------------------------

/// Tag: Get ARM memory base address and size.
pub const TAG_GET_ARM_MEMORY: u32 = 0x0001_0005;

/// Tag: Get board serial number.
pub const TAG_GET_BOARD_SERIAL: u32 = 0x0001_0004;

/// Tag: Set power state.
pub const TAG_SET_POWER_STATE: u32 = 0x0002_8001;

/// Tag: Request reboot.
pub const TAG_REBOOT: u32 = 0x0000_0003;

/// Tag: Request power off.
pub const TAG_POWER_OFF: u32 = 0x0000_0002;

// ---------------------------------------------------------------------------
// Property message constants
// ---------------------------------------------------------------------------

/// Request code in a property message.
pub const REQUEST_CODE: u32 = 0x0000_0000;

/// Response success code in a property message.
pub const RESPONSE_SUCCESS: u32 = 0x8000_0000;

/// End tag — signals the end of the tag list.
pub const END_TAG: u32 = 0x0000_0000;

// ---------------------------------------------------------------------------
// MailboxError
// ---------------------------------------------------------------------------

/// Errors that can occur during mailbox operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxError {
    /// The write FIFO was full and did not drain within the timeout.
    WriteTimeout,
    /// The read FIFO was empty and no response arrived within the timeout.
    ReadTimeout,
    /// The firmware returned a failure response code.
    ResponseError,
    /// The response payload indicates an error for the requested tag.
    TagError,
}

// ---------------------------------------------------------------------------
// MailboxMessage
// ---------------------------------------------------------------------------

/// Maximum number of `u32` words in a property message buffer.
///
/// This is large enough for any single-tag query used by this crate.
/// Complex multi-tag messages may require a larger buffer.
pub const MAILBOX_MESSAGE_MAX_WORDS: usize = 32;

/// A property message buffer for the BCM2712 VideoCore mailbox.
///
/// The buffer stores the full request/response message as an array of
/// `u32` words. The caller initialises the words with the desired tags,
/// then passes the message to [`send_property_message`] which blocks
/// until the firmware responds.
///
/// # Layout
///
/// | Index | Contents                                     |
/// |-------|----------------------------------------------|
/// | 0     | Total buffer size in bytes                   |
/// | 1     | Request code (`0x00000000`)                  |
/// | 2…N-1 | Tag entries                                  |
/// | N     | End tag (`0x00000000`)                       |
///
/// After [`send_property_message`] returns, word 1 contains the
/// response code and tag value buffers are overwritten with results.
#[derive(Debug)]
pub struct MailboxMessage {
    /// Message buffer — `u32` words, little-endian on the wire.
    pub buffer: [u32; MAILBOX_MESSAGE_MAX_WORDS],
}

impl MailboxMessage {
    /// Creates a new, zeroed message buffer.
    #[inline]
    pub const fn new() -> Self {
        Self {
            buffer: [0u32; MAILBOX_MESSAGE_MAX_WORDS],
        }
    }

    /// Returns the total buffer size in bytes as stored in word 0.
    #[inline]
    pub fn size(&self) -> u32 {
        self.buffer[0]
    }

    /// Sets the total buffer size in bytes in word 0.
    #[inline]
    pub fn set_size(&mut self, size: u32) {
        self.buffer[0] = size;
    }

    /// Returns the response code from word 1.
    #[inline]
    pub fn response_code(&self) -> u32 {
        self.buffer[1]
    }

    /// Returns `true` if the firmware acknowledged the request
    /// successfully (bit 31 of the response code is set).
    #[inline]
    pub fn is_response_success(&self) -> bool {
        (self.response_code() & RESPONSE_SUCCESS) != 0
    }

    /// Initialises a single-tag "Get ARM Memory" message.
    ///
    /// Tag: `TAG_GET_ARM_MEMORY` (0x00010005).
    /// Value buffer: 8 bytes (2 × u32 — base, size).
    pub fn init_get_arm_memory(&mut self) {
        // Word layout:
        // [0] total size  = 6 words × 4 = 24 bytes (header+tag+end)
        // [1] request code = 0
        // [2] tag = GET_ARM_MEMORY
        // [3] value buffer size = 8
        // [4] request/response code = 0
        // [5..6] value buffer (base + size, filled by firmware)
        // [7] end tag = 0
        self.buffer = [0u32; MAILBOX_MESSAGE_MAX_WORDS];
        self.buffer[0] = 32; // 8 words × 4 bytes
        self.buffer[1] = REQUEST_CODE;
        self.buffer[2] = TAG_GET_ARM_MEMORY;
        self.buffer[3] = 8; // value buffer size
        self.buffer[4] = 0; // request code
        // buffer[5] and buffer[6] will be filled by firmware
        self.buffer[7] = END_TAG;
    }

    /// Initialises a single-tag "Get Board Serial" message.
    ///
    /// Tag: `TAG_GET_BOARD_SERIAL` (0x00010004).
    /// Value buffer: 8 bytes (1 × u64 serial number).
    pub fn init_get_board_serial(&mut self) {
        self.buffer = [0u32; MAILBOX_MESSAGE_MAX_WORDS];
        self.buffer[0] = 32; // 8 words × 4 bytes
        self.buffer[1] = REQUEST_CODE;
        self.buffer[2] = TAG_GET_BOARD_SERIAL;
        self.buffer[3] = 8; // value buffer size
        self.buffer[4] = 0; // request code
        // buffer[5] and buffer[6] will be filled by firmware
        self.buffer[7] = END_TAG;
    }
}

impl Default for MailboxMessage {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Low-level register access (gated on aarch64)
// ---------------------------------------------------------------------------

/// Reads a 32-bit mailbox register at the given offset from MAILBOX_BASE.
///
/// Only available on `aarch64` targets.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn reg_read(offset: Address) -> u32 {
    let addr = MAILBOX_BASE + offset;
    // SAFETY: Caller guarantees `addr` is a valid, aligned MMIO register
    // within the BCM2712 mailbox register space.
    unsafe { ptr::read_volatile(addr as *const u32) }
}

/// Writes a 32-bit value to a mailbox register at the given offset from
/// MAILBOX_BASE.
///
/// Only available on `aarch64` targets.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn reg_write(offset: Address, value: u32) {
    let addr = MAILBOX_BASE + offset;
    // SAFETY: Caller guarantees `addr` is a valid, aligned MMIO register
    // within the BCM2712 mailbox register space.
    unsafe { ptr::write_volatile(addr as *mut u32, value) }
}

// ---------------------------------------------------------------------------
// Stub implementations for non-aarch64 targets
// ---------------------------------------------------------------------------

/// Stub register read for non-aarch64 targets (always returns 0).
///
/// Kept for completeness but unused on hosted targets since all
/// mailbox operations have their own stubs.
#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
#[allow(dead_code)]
fn reg_read(_offset: Address) -> u32 {
    0
}

/// Stub register write for non-aarch64 targets (no-op).
///
/// Kept for completeness but unused on hosted targets since all
/// mailbox operations have their own stubs.
#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
#[allow(dead_code)]
fn reg_write(_offset: Address, _value: u32) {}

// ---------------------------------------------------------------------------
// Timeout loop count
// ---------------------------------------------------------------------------

/// Number of spin-loop iterations before declaring a mailbox timeout.
///
/// This is a rough heuristic; on a 2.4 GHz Cortex-A76 each iteration
/// is roughly 1–2 ns, so this gives ~100–200 ms of patience.
pub const MAILBOX_TIMEOUT: u32 = 100_000_000;

// ---------------------------------------------------------------------------
// Core mailbox send/receive
// ---------------------------------------------------------------------------

/// Sends a property message to the VideoCore firmware via channel 8 and
/// waits for the response.
///
/// The message buffer must be 16-byte aligned (the BCM2712 mailbox
/// interface requires the address to be 16-byte aligned with the upper
/// 28 bits forming the payload). In practice, `MailboxMessage` is
/// stack-allocated and Rust's default alignment for `[u32; N]` satisfies
/// this when N ≥ 4.
///
/// # Protocol
///
/// 1. Wait until MAILBOX1_STATUS indicates the write FIFO is not full.
/// 2. Write `(buffer_physical_address >> 4) | channel` to MAILBOX1_WRITE.
/// 3. Wait until MAILBOX0_STATUS indicates the read FIFO is not empty.
/// 4. Read from MAILBOX0_READ; verify the channel matches.
/// 5. Check the response code in the message buffer.
///
/// # Errors
///
/// Returns [`MailboxError::WriteTimeout`] if the write FIFO never
/// drains, [`MailboxError::ReadTimeout`] if no response arrives, or
/// [`MailboxError::ResponseError`] if the firmware responds with an
/// error.
///
/// # Note on physical addresses
///
/// On aarch64 bare-metal with identity-mapped RAM, the physical address
/// of the buffer equals its virtual address. This function assumes
/// identity mapping.
#[cfg(target_arch = "aarch64")]
pub fn send_property_message(msg: &mut MailboxMessage) -> Result<(), MailboxError> {
    // Compute the physical address of the buffer.
    let buf_addr = &msg.buffer as *const _ as u64;

    // Ensure the buffer address is 16-byte aligned.
    assert!(
        buf_addr & 0xF == 0,
        "mailbox buffer must be 16-byte aligned"
    );

    // Build the combined value: upper 28 bits of the address (shifted
    // right by 4) in bits [31:4], channel number in bits [3:0].
    let combined = ((buf_addr >> 4) as u32) | CHANNEL_PROPERTY;

    // 1. Wait until the write FIFO is not full.
    let mut timeout = MAILBOX_TIMEOUT;
    while (reg_read(MAILBOX1_STATUS) & STATUS_FULL) != 0 {
        if timeout == 0 {
            return Err(MailboxError::WriteTimeout);
        }
        timeout -= 1;
        core::hint::spin_loop();
    }

    // 2. Send the message.
    reg_write(MAILBOX1_WRITE, combined);

    // 3. Wait until the read FIFO is not empty and the response is on
    //    the property channel.
    timeout = MAILBOX_TIMEOUT;
    loop {
        if (reg_read(MAILBOX0_STATUS) & STATUS_EMPTY) == 0 {
            let response = reg_read(MAILBOX0_READ);
            // The low 4 bits contain the channel; verify it matches.
            if (response & 0xF) == CHANNEL_PROPERTY {
                break;
            }
            // Wrong channel — discard and keep waiting.
        }
        if timeout == 0 {
            return Err(MailboxError::ReadTimeout);
        }
        timeout -= 1;
        core::hint::spin_loop();
    }

    // 4. Check the firmware response code.
    if !msg.is_response_success() {
        return Err(MailboxError::ResponseError);
    }

    Ok(())
}

/// Sends a property message (stub for non-aarch64 targets).
///
/// Always returns `Ok(())` on host / non-aarch64 builds.
#[cfg(not(target_arch = "aarch64"))]
pub fn send_property_message(_msg: &mut MailboxMessage) -> Result<(), MailboxError> {
    // Stub: simulate success on non-aarch64 targets.
    Ok(())
}

// ---------------------------------------------------------------------------
// High-level property queries
// ---------------------------------------------------------------------------

/// Queries the VideoCore firmware for the ARM memory layout.
///
/// Returns `(base_address, size)` in bytes. On a Pi 5 with 4 GiB RAM,
/// the typical result is `(0, 0x1_0000_0000)`.
///
/// # Errors
///
/// Propagates any [`MailboxError`] from [`send_property_message`], or
/// returns [`MailboxError::TagError`] if the tag response indicates
/// failure.
#[cfg(target_arch = "aarch64")]
pub fn get_arm_memory() -> Result<(u64, u64), MailboxError> {
    let mut msg = MailboxMessage::new();
    msg.init_get_arm_memory();
    send_property_message(&mut msg)?;

    // Check the tag response code (bit 31 set = response present).
    if (msg.buffer[4] & 0x8000_0000) == 0 {
        return Err(MailboxError::TagError);
    }

    let base = msg.buffer[5] as u64;
    let size = msg.buffer[6] as u64;
    Ok((base, size))
}

/// Returns the ARM memory layout (stub for non-aarch64 targets).
///
/// Returns a default `(0, 0x4000_0000)` (1 GiB) on non-aarch64 builds.
#[cfg(not(target_arch = "aarch64"))]
pub fn get_arm_memory() -> Result<(u64, u64), MailboxError> {
    // Stub: return 1 GiB at address 0.
    Ok((0, 0x4000_0000))
}

/// Queries the VideoCore firmware for the board serial number.
///
/// Returns a 64-bit serial number unique to this Pi 5 board.
///
/// # Errors
///
/// Propagates any [`MailboxError`] from [`send_property_message`], or
/// returns [`MailboxError::TagError`] if the tag response indicates
/// failure.
#[cfg(target_arch = "aarch64")]
pub fn get_board_serial() -> Result<u64, MailboxError> {
    let mut msg = MailboxMessage::new();
    msg.init_get_board_serial();
    send_property_message(&mut msg)?;

    // Check the tag response code.
    if (msg.buffer[4] & 0x8000_0000) == 0 {
        return Err(MailboxError::TagError);
    }

    // The serial number is a 64-bit value across two u32 words.
    let lo = msg.buffer[5] as u64;
    let hi = msg.buffer[6] as u64;
    Ok((hi << 32) | lo)
}

/// Returns the board serial number (stub for non-aarch64 targets).
///
/// Returns `0` on non-aarch64 builds.
#[cfg(not(target_arch = "aarch64"))]
pub fn get_board_serial() -> Result<u64, MailboxError> {
    // Stub: return a dummy serial.
    Ok(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test 1: MailboxMessage::init_get_arm_memory layout
    // -----------------------------------------------------------------------
    #[test]
    fn mailbox_message_init_get_arm_memory() {
        let mut msg = MailboxMessage::new();
        msg.init_get_arm_memory();

        // Total size: 8 words × 4 bytes = 32
        assert_eq!(msg.buffer[0], 32, "total size should be 32 bytes");
        // Request code
        assert_eq!(msg.buffer[1], REQUEST_CODE, "request code should be 0");
        // Tag identity
        assert_eq!(msg.buffer[2], TAG_GET_ARM_MEMORY, "tag should be GET_ARM_MEMORY");
        // Value buffer size: 8 bytes
        assert_eq!(msg.buffer[3], 8, "value buffer size should be 8");
        // Request/response code
        assert_eq!(msg.buffer[4], 0, "tag request code should be 0");
        // End tag
        assert_eq!(msg.buffer[7], END_TAG, "end tag should be 0");
    }

    // -----------------------------------------------------------------------
    // Test 2: MailboxMessage::init_get_board_serial layout
    // -----------------------------------------------------------------------
    #[test]
    fn mailbox_message_init_get_board_serial() {
        let mut msg = MailboxMessage::new();
        msg.init_get_board_serial();

        assert_eq!(msg.buffer[0], 32, "total size should be 32 bytes");
        assert_eq!(msg.buffer[1], REQUEST_CODE, "request code should be 0");
        assert_eq!(msg.buffer[2], TAG_GET_BOARD_SERIAL, "tag should be GET_BOARD_SERIAL");
        assert_eq!(msg.buffer[3], 8, "value buffer size should be 8");
        assert_eq!(msg.buffer[4], 0, "tag request code should be 0");
        assert_eq!(msg.buffer[7], END_TAG, "end tag should be 0");
    }

    // -----------------------------------------------------------------------
    // Test 3: MailboxMessage default and is_response_success
    // -----------------------------------------------------------------------
    #[test]
    fn mailbox_message_default_and_response_check() {
        let msg = MailboxMessage::default();
        // All-zero buffer: response code is 0 → not success.
        assert!(!msg.is_response_success(), "zeroed buffer should not indicate success");

        let mut msg = MailboxMessage::new();
        msg.buffer[1] = RESPONSE_SUCCESS;
        assert!(msg.is_response_success(), "response success bit should be set");
    }

    // -----------------------------------------------------------------------
    // Test 4: Constant values are correct
    // -----------------------------------------------------------------------
    #[test]
    fn mailbox_constants() {
        assert_eq!(MAILBOX_BASE, 0x1C00_0000);
        assert_eq!(MAILBOX0_READ, 0x00);
        assert_eq!(MAILBOX0_STATUS, 0x18);
        assert_eq!(MAILBOX1_WRITE, 0x20);
        assert_eq!(MAILBOX1_STATUS, 0x38);
        assert_eq!(STATUS_EMPTY, 0x4000_0000);
        assert_eq!(STATUS_FULL, 0x8000_0000);
        assert_eq!(CHANNEL_PROPERTY, 8);
    }

    // -----------------------------------------------------------------------
    // Test 5: Stub get_arm_memory on non-aarch64
    // -----------------------------------------------------------------------
    #[test]
    fn stub_get_arm_memory() {
        let result = get_arm_memory();
        assert!(result.is_ok(), "stub get_arm_memory should return Ok");
        let (base, size) = result.unwrap();
        assert_eq!(base, 0, "stub base should be 0");
        assert_eq!(size, 0x4000_0000, "stub size should be 1 GiB");
    }

    // -----------------------------------------------------------------------
    // Test 6: Stub get_board_serial on non-aarch64
    // -----------------------------------------------------------------------
    #[test]
    fn stub_get_board_serial() {
        let result = get_board_serial();
        assert!(result.is_ok(), "stub get_board_serial should return Ok");
        assert_eq!(result.unwrap(), 0, "stub serial should be 0");
    }

    // -----------------------------------------------------------------------
    // Test 7: send_property_message stub returns Ok on non-aarch64
    // -----------------------------------------------------------------------
    #[test]
    fn stub_send_property_message() {
        let mut msg = MailboxMessage::new();
        msg.init_get_arm_memory();
        let result = send_property_message(&mut msg);
        assert!(result.is_ok(), "stub send_property_message should return Ok");
    }
}
