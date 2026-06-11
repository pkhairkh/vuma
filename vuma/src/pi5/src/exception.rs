//! Exception handling for the BCM2712 (Raspberry Pi 5).
//!
//! This module provides the Rust-side exception handler infrastructure for
//! the ARM64 exception vector table on the BCM2712. It defines:
//!
//! - [`ExceptionContext`] — saved CPU state on exception entry
//! - [`ExceptionType`] — enumeration of the four AArch64 exception classes
//! - Handler functions: [`handle_sync`], [`handle_irq`], [`handle_fiq`], [`handle_serror`]
//! - [`install_handlers`] — writes the vector base address to `VBAR_EL1`
//!
//! # Exception Vector Table
//!
//! The full AArch64 vector table has 16 entries, but they all map to one of
//! four exception classes. The assembly entry points in [`boot`](crate::boot)
//! save the full CPU context, call the appropriate handler here, restore
//! context, and execute `ERET`.
//!
//! # Overriding Handlers
//!
//! The default handlers park the core in a `WFE` loop. To install custom
//! handlers, replace the function pointers or re-implement the handler
//! functions.

// ---------------------------------------------------------------------------
// ExceptionContext
// ---------------------------------------------------------------------------

/// Saved CPU state passed to exception handlers.
///
/// This struct matches the layout pushed onto the stack by the assembly
/// exception entry points in [`boot`](crate::boot). It is `#[repr(C)]`
/// to guarantee field ordering and no padding surprises.
///
/// | Field  | Content                                      |
/// |--------|----------------------------------------------|
/// | `x[0]`–`x[30]` | General-purpose registers x0–x30     |
/// | `spsr` | Saved Program Status Register (EL1)          |
/// | `elr`  | Exception Link Register (return address)      |
/// | `esr`  | Exception Syndrome Register (fault info)      |
/// | `far`  | Fault Address Register (faulting address)     |
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ExceptionContext {
    /// General-purpose registers x0–x30.
    pub x: [u64; 31],
    /// Saved Program Status Register — the PSTATE at the time of exception.
    pub spsr: u64,
    /// Exception Link Register — the address to return to after handling.
    pub elr: u64,
    /// Exception Syndrome Register — encodes the reason for the exception.
    pub esr: u64,
    /// Fault Address Register — the address that caused a fault (if any).
    pub far: u64,
}

impl ExceptionContext {
    /// Creates a new zeroed `ExceptionContext`.
    #[inline]
    pub const fn new() -> Self {
        Self {
            x: [0u64; 31],
            spsr: 0,
            elr: 0,
            esr: 0,
            far: 0,
        }
    }

    /// Returns the ESR Exception Class field (bits [31:26]).
    ///
    /// The EC field identifies the type of exception:
    ///
    /// | EC    | Description                          |
    /// |-------|--------------------------------------|
    /// | 0x00  | Unknown reason                       |
    /// | 0x01  | Trapped WFI/WFE                      |
    /// | 0x07  | Access to SIMD/FP from EL0           |
    /// | 0x0E  | Illegal Execution state              |
    /// | 0x15  | SVC instruction execution in AArch64 |
    /// | 0x21  | Instruction abort from lower EL      |
    /// | 0x25  | Data abort from lower EL             |
    /// | 0x22  | Instruction abort from same EL       |
    /// | 0x26  | Data abort from same EL              |
    /// | 0x30  | Breakpoint from lower EL             |
    /// | 0x34  | Breakpoint from same EL              |
    #[inline]
    pub fn esr_ec(&self) -> u32 {
        ((self.esr >> 26) & 0x3F) as u32
    }

    /// Returns the ESR Instruction Specific Syndrome field (bits [24:0]).
    ///
    /// The ISS field provides additional information about the exception,
    /// whose interpretation depends on the EC field.
    #[inline]
    pub fn esr_iss(&self) -> u32 {
        (self.esr & 0x01FF_FFFF) as u32
    }

    /// Returns the ESR Conditional field (bits [25:24] when EC indicates
    /// a conditional instruction).
    #[inline]
    pub fn esr_cond(&self) -> u32 {
        ((self.esr >> 20) & 0xF) as u32
    }

    /// Returns `true` if this was a data abort from the same EL (EC = 0x26)
    /// or from a lower EL (EC = 0x25).
    #[inline]
    pub fn is_data_abort(&self) -> bool {
        let ec = self.esr_ec();
        ec == 0x25 || ec == 0x26
    }

    /// Returns `true` if this was an instruction abort (EC = 0x21 or 0x22).
    #[inline]
    pub fn is_instruction_abort(&self) -> bool {
        let ec = self.esr_ec();
        ec == 0x21 || ec == 0x22
    }
}

// ---------------------------------------------------------------------------
// ExceptionType
// ---------------------------------------------------------------------------

/// Enumeration of the four AArch64 exception classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionType {
    /// Synchronous exception — e.g. data/instruction abort, SVC, undefined instruction.
    Synchronous,
    /// IRQ (Interrupt Request) — standard hardware interrupt.
    Irq,
    /// FIQ (Fast Interrupt Request) — high-priority hardware interrupt.
    Fiq,
    /// SError (System Error) — asynchronous external abort.
    SError,
}

impl ExceptionType {
    /// Returns a static string name for the exception type.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ExceptionType::Synchronous => "Synchronous",
            ExceptionType::Irq => "IRQ",
            ExceptionType::Fiq => "FIQ",
            ExceptionType::SError => "SError",
        }
    }
}

impl core::fmt::Display for ExceptionType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Exception Handler Functions
// ---------------------------------------------------------------------------

/// Handler for synchronous exceptions.
///
/// Called by the assembly entry point when a synchronous exception occurs
/// (data abort, instruction abort, SVC, undefined instruction, etc.).
///
/// The default implementation parks the core in a `WFE` loop.
/// Override this function to install custom handling.
pub fn handle_sync(ctx: &mut ExceptionContext) {
    let _ = ctx;
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for IRQ (Interrupt Request) exceptions.
///
/// Called by the assembly entry point when a hardware IRQ is signalled.
///
/// The default implementation acknowledges any pending GIC interrupt
/// and returns. Override this function to install custom handling.
pub fn handle_irq(ctx: &mut ExceptionContext) {
    let _ = ctx;
    // Default: acknowledge and dismiss any pending GIC IRQ.
    // In a full kernel this would dispatch to device-specific handlers.
    let gic = crate::gic::Gic400::new();
    let irq = gic.acknowledge_irq();
    if irq < crate::gic::IAR_SPURIOUS {
        gic.end_of_irq(irq);
    }
}

/// Handler for FIQ (Fast Interrupt Request) exceptions.
///
/// Called by the assembly entry point when a high-priority FIQ is signalled.
/// The BCM2712 typically uses FIQ for GPU-related interrupts.
///
/// The default implementation parks the core in a `WFE` loop.
pub fn handle_fiq(ctx: &mut ExceptionContext) {
    let _ = ctx;
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for SError (System Error) exceptions.
///
/// Called by the assembly entry point when an asynchronous external abort
/// occurs. These are typically caused by memory system errors.
///
/// The default implementation parks the core in a `WFE` loop.
pub fn handle_serror(ctx: &mut ExceptionContext) {
    let _ = ctx;
    loop {
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// Install handlers
// ---------------------------------------------------------------------------

/// Installs the exception vector table by writing its base address to
/// `VBAR_EL1`.
///
/// This is equivalent to [`boot::install_exception_vector_table`] but
/// is provided here as a convenience for code that wants to reference
/// the exception module.
///
/// # Safety
///
/// Must be called only from EL1. The vector table must be aligned to
/// 2 KiB (0x800) as required by the architecture.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn install_handlers() {
    // SAFETY: The caller guarantees we are executing at EL1 and the
    // exception_vector_table function is properly aligned and contains
    // a valid vector table.
    core::arch::asm!(
        "adr x0, {table}",
        "msr vbar_el1, x0",
        "isb",
        table = sym crate::boot::exception_vector_table,
        out("x0") _,
        options(nostack, preserves_flags)
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::format;

    // -----------------------------------------------------------------------
    // Test 1: ExceptionContext size matches assembly layout
    // -----------------------------------------------------------------------
    #[test]
    fn exception_context_size() {
        // 31 * 8 (x0-x30) + 4 * 8 (spsr, elr, esr, far) = 280 bytes
        assert_eq!(
            core::mem::size_of::<ExceptionContext>(),
            31 * 8 + 4 * 8,
            "ExceptionContext should be 280 bytes"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: ExceptionContext default is all zeros
    // -----------------------------------------------------------------------
    #[test]
    fn exception_context_default_is_zero() {
        let ctx = ExceptionContext::default();
        assert!(ctx.x.iter().all(|&v| v == 0), "All x registers should be 0");
        assert_eq!(ctx.spsr, 0);
        assert_eq!(ctx.elr, 0);
        assert_eq!(ctx.esr, 0);
        assert_eq!(ctx.far, 0);
    }

    // -----------------------------------------------------------------------
    // Test 3: ESR EC field extraction
    // -----------------------------------------------------------------------
    #[test]
    fn esr_ec_extraction() {
        let mut ctx = ExceptionContext::default();
        // Data Abort from same EL: EC = 0x26 (bits [31:26])
        // ESR = 0x26 << 26 | ISS = 0x98000000
        ctx.esr = 0x9800_0000;
        assert_eq!(ctx.esr_ec(), 0x26);

        // SVC from AArch64: EC = 0x15
        ctx.esr = (0x15u64 << 26) | 0x01;
        assert_eq!(ctx.esr_ec(), 0x15);

        // Unknown: EC = 0x00
        ctx.esr = 0;
        assert_eq!(ctx.esr_ec(), 0x00);
    }

    // -----------------------------------------------------------------------
    // Test 4: ESR ISS field extraction
    // -----------------------------------------------------------------------
    #[test]
    fn esr_iss_extraction() {
        let mut ctx = ExceptionContext::default();
        ctx.esr = 0x9800_1234;
        assert_eq!(ctx.esr_iss(), 0x1234);

        ctx.esr = 0x9800_0000;
        assert_eq!(ctx.esr_iss(), 0x0000);

        // ISS uses bits [24:0], max value = 0x01FF_FFFF
        ctx.esr = (0x26u64 << 26) | 0x01FF_FFFF;
        assert_eq!(ctx.esr_iss(), 0x01FF_FFFF);
    }

    // -----------------------------------------------------------------------
    // Test 5: ExceptionType Display and as_str
    // -----------------------------------------------------------------------
    #[test]
    fn exception_type_display() {
        assert_eq!(ExceptionType::Synchronous.as_str(), "Synchronous");
        assert_eq!(ExceptionType::Irq.as_str(), "IRQ");
        assert_eq!(ExceptionType::Fiq.as_str(), "FIQ");
        assert_eq!(ExceptionType::SError.as_str(), "SError");

        // Display trait
        assert_eq!(format!("{}", ExceptionType::Irq), "IRQ");
        assert_eq!(format!("{}", ExceptionType::Synchronous), "Synchronous");
    }

    // -----------------------------------------------------------------------
    // Test 6: is_data_abort / is_instruction_abort helpers
    // -----------------------------------------------------------------------
    #[test]
    fn exception_context_abort_helpers() {
        let mut ctx = ExceptionContext::default();

        // Data abort from lower EL: EC = 0x25
        ctx.esr = 0x25u64 << 26;
        assert!(ctx.is_data_abort());
        assert!(!ctx.is_instruction_abort());

        // Data abort from same EL: EC = 0x26
        ctx.esr = 0x26u64 << 26;
        assert!(ctx.is_data_abort());
        assert!(!ctx.is_instruction_abort());

        // Instruction abort from lower EL: EC = 0x21
        ctx.esr = 0x21u64 << 26;
        assert!(!ctx.is_data_abort());
        assert!(ctx.is_instruction_abort());

        // Instruction abort from same EL: EC = 0x22
        ctx.esr = 0x22u64 << 26;
        assert!(!ctx.is_data_abort());
        assert!(ctx.is_instruction_abort());

        // SVC: neither abort
        ctx.esr = 0x15u64 << 26;
        assert!(!ctx.is_data_abort());
        assert!(!ctx.is_instruction_abort());
    }

    // -----------------------------------------------------------------------
    // Test 7: ExceptionContext::new() is const and matches default
    // -----------------------------------------------------------------------
    #[test]
    fn exception_context_new_matches_default() {
        const CTX_NEW: ExceptionContext = ExceptionContext::new();
        let ctx_default = ExceptionContext::default();
        assert_eq!(CTX_NEW.x, ctx_default.x);
        assert_eq!(CTX_NEW.spsr, ctx_default.spsr);
        assert_eq!(CTX_NEW.elr, ctx_default.elr);
        assert_eq!(CTX_NEW.esr, ctx_default.esr);
        assert_eq!(CTX_NEW.far, ctx_default.far);
    }
}
