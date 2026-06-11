//! Power management module for the Raspberry Pi 5 (BCM2712).
//!
//! Provides basic power-control primitives using the VideoCore mailbox
//! property interface. The BCM2712 firmware manages device power rails;
//! the ARM side requests power-state changes via mailbox tags.
//!
//! # Device IDs
//!
//! | Constant         | Value | Device        |
//! |------------------|-------|---------------|
//! | [`POWER_SD`]     | 0     | SD card       |
//! | [`POWER_UART0`]  | 1     | UART0 (PL011) |
//! | [`POWER_USB`]    | 3     | USB           |
//!
//! # Usage
//!
//! ```no_run
//! use vuma_pi5::power::{set_power_state, POWER_SD, POWER_UART0};
//!
//! // Power on the SD card controller.
//! set_power_state(POWER_SD, true).expect("failed to power on SD");
//!
//! // Power off UART0.
//! set_power_state(POWER_UART0, false).expect("failed to power off UART0");
//! ```

use crate::mailbox::MailboxError;

#[cfg(target_arch = "aarch64")]
use crate::mailbox::{send_property_message, MailboxMessage, REQUEST_CODE, END_TAG, TAG_SET_POWER_STATE};

// ---------------------------------------------------------------------------
// Device power IDs
// ---------------------------------------------------------------------------

/// SD card controller power domain.
pub const POWER_SD: u32 = 0;

/// UART0 (PL011) power domain.
pub const POWER_UART0: u32 = 1;

/// USB controller power domain.
pub const POWER_USB: u32 = 3;

// ---------------------------------------------------------------------------
// Power state flags (used in the SET_POWER_STATE tag value buffer)
// ---------------------------------------------------------------------------

/// Bit indicating the device should be powered on.
pub const POWER_STATE_ON: u32 = 1;

/// Bit indicating the device should be powered off.
pub const POWER_STATE_OFF: u32 = 0;

/// Bit requesting that the firmware wait for the power state change
/// to complete before responding.
pub const POWER_STATE_WAIT: u32 = 0x2;

// ---------------------------------------------------------------------------
// PowerError
// ---------------------------------------------------------------------------

/// Errors that can occur during power management operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerError {
    /// The mailbox transaction failed.
    Mailbox(MailboxError),
    /// The firmware returned an error for the SET_POWER_STATE tag.
    TagError,
    /// The device did not reach the requested power state.
    StateMismatch,
}

impl From<MailboxError> for PowerError {
    fn from(err: MailboxError) -> Self {
        PowerError::Mailbox(err)
    }
}

// ---------------------------------------------------------------------------
// set_power_state
// ---------------------------------------------------------------------------

/// Requests the VideoCore firmware to change the power state of a device.
///
/// # Arguments
///
/// * `device` — Device ID (e.g. [`POWER_SD`], [`POWER_UART0`], [`POWER_USB`]).
/// * `on` — `true` to power on, `false` to power off.
///
/// # Errors
///
/// Returns [`PowerError::Mailbox`] if the mailbox transaction fails,
/// [`PowerError::TagError`] if the tag response is invalid, or
/// [`PowerError::StateMismatch`] if the device did not reach the
/// requested state.
#[cfg(target_arch = "aarch64")]
pub fn set_power_state(device: u32, on: bool) -> Result<(), PowerError> {
    let mut msg = MailboxMessage::new();

    let desired_state = if on {
        POWER_STATE_ON | POWER_STATE_WAIT
    } else {
        POWER_STATE_OFF | POWER_STATE_WAIT
    };

    // Word layout:
    // [0] total size = 8 words × 4 = 32 bytes
    // [1] request code = 0
    // [2] tag = SET_POWER_STATE
    // [3] value buffer size = 8
    // [4] request/response code
    // [5] device ID
    // [6] desired state
    // [7] end tag
    msg.buffer[0] = 32;
    msg.buffer[1] = REQUEST_CODE;
    msg.buffer[2] = TAG_SET_POWER_STATE;
    msg.buffer[3] = 8;
    msg.buffer[4] = 0;
    msg.buffer[5] = device;
    msg.buffer[6] = desired_state;
    msg.buffer[7] = END_TAG;

    send_property_message(&mut msg).map_err(PowerError::Mailbox)?;

    // Check the tag response code (bit 31 set = response present).
    if (msg.buffer[4] & 0x8000_0000) == 0 {
        return Err(PowerError::TagError);
    }

    // The firmware writes the actual state back into word [6].
    // Bit 0 indicates whether the device is on; bit 1 indicates
    // the existence of the device.
    let actual_state = msg.buffer[6];
    let is_on = (actual_state & POWER_STATE_ON) != 0;
    if is_on != on {
        return Err(PowerError::StateMismatch);
    }

    Ok(())
}

/// Requests a power state change (stub for non-aarch64 targets).
///
/// Always returns `Ok(())` on non-aarch64 builds.
#[cfg(not(target_arch = "aarch64"))]
pub fn set_power_state(_device: u32, _on: bool) -> Result<(), PowerError> {
    Ok(())
}

// ---------------------------------------------------------------------------
// reboot
// ---------------------------------------------------------------------------

/// Reboots the Raspberry Pi 5 via the VideoCore mailbox.
///
/// This function sends a reboot command to the firmware and **never
/// returns**. If the mailbox call fails the function enters an
/// infinite loop (halt).
///
/// # Safety
///
/// This is a destructive operation — all running code is terminated
/// when the firmware processes the reboot request.
#[cfg(target_arch = "aarch64")]
pub fn reboot() -> ! {
    let mut msg = MailboxMessage::new();

    // Word layout:
    // [0] total size = 8 words × 4 = 32 bytes
    // [1] request code = 0
    // [2] tag = REBOOT (0x00000003)
    // [3] value buffer size = 8
    // [4] request/response code = 0
    // [5] reboot code = 1
    // [6] 0 (padding)
    // [7] end tag
    msg.buffer[0] = 32;
    msg.buffer[1] = REQUEST_CODE;
    msg.buffer[2] = crate::mailbox::TAG_REBOOT;
    msg.buffer[3] = 8;
    msg.buffer[4] = 0;
    msg.buffer[5] = 1; // reboot code
    msg.buffer[6] = 0;
    msg.buffer[7] = END_TAG;

    // Best-effort send; if it fails we halt anyway.
    let _ = send_property_message(&mut msg);

    // If we reach here, the reboot did not take effect. Halt.
    loop {
        core::hint::spin_loop();
    }
}

/// Reboot stub for non-aarch64 targets.
///
/// Since bare-metal `-> !` functions cannot meaningfully exist on
/// hosted targets, this stub panics.
#[cfg(not(target_arch = "aarch64"))]
pub fn reboot() -> ! {
    panic!("reboot() is not available on non-aarch64 targets");
}

// ---------------------------------------------------------------------------
// power_off
// ---------------------------------------------------------------------------

/// Powers off the Raspberry Pi 5 via the VideoCore mailbox.
///
/// This function sends a power-off command to the firmware and **never
/// returns**. If the mailbox call fails the function enters an
/// infinite loop (halt).
///
/// # Safety
///
/// This is a destructive operation — all running code is terminated
/// when the firmware processes the power-off request.
#[cfg(target_arch = "aarch64")]
pub fn power_off() -> ! {
    let mut msg = MailboxMessage::new();

    // Word layout:
    // [0] total size = 8 words × 4 = 32 bytes
    // [1] request code = 0
    // [2] tag = POWER_OFF (0x00000002)
    // [3] value buffer size = 8
    // [4] request/response code = 0
    // [5] power-off code = 0
    // [6] 0 (padding)
    // [7] end tag
    msg.buffer[0] = 32;
    msg.buffer[1] = REQUEST_CODE;
    msg.buffer[2] = crate::mailbox::TAG_POWER_OFF;
    msg.buffer[3] = 8;
    msg.buffer[4] = 0;
    msg.buffer[5] = 0; // power-off code
    msg.buffer[6] = 0;
    msg.buffer[7] = END_TAG;

    // Best-effort send; if it fails we halt anyway.
    let _ = send_property_message(&mut msg);

    // If we reach here, the power-off did not take effect. Halt.
    loop {
        core::hint::spin_loop();
    }
}

/// Power-off stub for non-aarch64 targets.
///
/// Since bare-metal `-> !` functions cannot meaningfully exist on
/// hosted targets, this stub panics.
#[cfg(not(target_arch = "aarch64"))]
pub fn power_off() -> ! {
    panic!("power_off() is not available on non-aarch64 targets");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::{MailboxMessage, REQUEST_CODE, END_TAG, TAG_SET_POWER_STATE};

    // -----------------------------------------------------------------------
    // Test 1: Device ID constants have expected values
    // -----------------------------------------------------------------------
    #[test]
    fn device_id_constants() {
        assert_eq!(POWER_SD, 0);
        assert_eq!(POWER_UART0, 1);
        assert_eq!(POWER_USB, 3);
    }

    // -----------------------------------------------------------------------
    // Test 2: Power state flags
    // -----------------------------------------------------------------------
    #[test]
    fn power_state_flags() {
        assert_eq!(POWER_STATE_ON, 1);
        assert_eq!(POWER_STATE_OFF, 0);
        assert_eq!(POWER_STATE_WAIT, 0x2);

        // The "on" request should include WAIT.
        let on_state = POWER_STATE_ON | POWER_STATE_WAIT;
        assert_eq!(on_state, 0x3, "ON | WAIT should be 0x3");

        let off_state = POWER_STATE_OFF | POWER_STATE_WAIT;
        assert_eq!(off_state, 0x2, "OFF | WAIT should be 0x2");
    }

    // -----------------------------------------------------------------------
    // Test 3: set_power_state stub returns Ok on non-aarch64
    // -----------------------------------------------------------------------
    #[test]
    fn stub_set_power_state() {
        assert!(
            set_power_state(POWER_SD, true).is_ok(),
            "stub set_power_state should return Ok"
        );
        assert!(
            set_power_state(POWER_UART0, false).is_ok(),
            "stub set_power_state (off) should return Ok"
        );
        assert!(
            set_power_state(POWER_USB, true).is_ok(),
            "stub set_power_state (USB) should return Ok"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: PowerError::from(MailboxError)
    // -----------------------------------------------------------------------
    #[test]
    fn power_error_from_mailbox_error() {
        let mb_err = MailboxError::WriteTimeout;
        let pwr_err: PowerError = mb_err.into();
        assert_eq!(pwr_err, PowerError::Mailbox(MailboxError::WriteTimeout));
    }

    // -----------------------------------------------------------------------
    // Test 5: MailboxMessage for SET_POWER_STATE has correct layout
    // -----------------------------------------------------------------------
    #[test]
    fn set_power_state_message_layout() {
        // Manually construct the message that set_power_state would build
        // and verify the layout.
        let mut msg = MailboxMessage::new();
        let desired_state = POWER_STATE_ON | POWER_STATE_WAIT;
        msg.buffer[0] = 32;
        msg.buffer[1] = REQUEST_CODE;
        msg.buffer[2] = TAG_SET_POWER_STATE;
        msg.buffer[3] = 8;
        msg.buffer[4] = 0;
        msg.buffer[5] = POWER_SD;
        msg.buffer[6] = desired_state;
        msg.buffer[7] = END_TAG;

        assert_eq!(msg.buffer[0], 32, "total size should be 32");
        assert_eq!(msg.buffer[2], TAG_SET_POWER_STATE, "tag should be SET_POWER_STATE");
        assert_eq!(msg.buffer[3], 8, "value buffer size should be 8");
        assert_eq!(msg.buffer[5], POWER_SD, "device should be SD");
        assert_eq!(msg.buffer[6], 0x3, "desired state should be ON|WAIT");
        assert_eq!(msg.buffer[7], END_TAG, "end tag should be 0");
    }
}
