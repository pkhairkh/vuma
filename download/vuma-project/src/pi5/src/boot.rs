//! Bare-metal boot code for the Raspberry Pi 5 (BCM2712).
//!
//! This module provides the earliest initialisation code that runs on the
//! Cortex-A76 cores after the VideoCore bootloader hands off control. It
//! is responsible for:
//!
//! * Setting up the stack pointer for each core (16 KiB, 16-byte aligned).
//! * Zeroing the BSS section.
//! * Installing the ARM64 exception vector table.
//! * Parsing the Flattened Device Tree (FDT / DTB) passed in `x0`.
//! * Routing core 0 to [`boot_main`] while parking cores 1–3 in a `WFE` loop.
//!
//! # Memory layout at entry
//!
//! The Pi 5 bootloader loads the kernel image at physical address `0x80000`
//! and jumps to that address with:
//!
//! * `x0` — physical address of the DTB (Flattened Device Tree).
//! * `x1` – `x3` — zero (unused by convention).
//!
//! # Exception vector table
//!
//! The full AArch64 vector table contains 16 entries grouped by exception
//! level and stack pointer selection. This module defines handler stubs for
//! all entries; they currently spin-loop so the system halts visibly rather
//! than executing undefined instructions.

use crate::platform::{PERIPHERAL_BASE, UART_BASE_OFFSET};
use crate::smp::CoreId;
use crate::uart::Uart;

// ---------------------------------------------------------------------------
// Linker symbols (provided by the linker script)
// ---------------------------------------------------------------------------

extern "C" {
    /// Start of the BSS section (defined in the linker script).
    static __bss_start: u8;
    /// End of the BSS section (defined in the linker script).
    static __bss_end: u8;
}

// ---------------------------------------------------------------------------
// Boot constants
// ---------------------------------------------------------------------------

/// Physical address where the Pi 5 bootloader loads and jumps to the kernel.
pub const KERNEL_ENTRY: usize = 0x80_000;

/// Stack size per core in bytes (16 KiB).
pub const STACK_SIZE_PER_CORE: usize = 16 * 1024;

/// Stack alignment in bytes. AArch64 AAPCS mandates 16-byte alignment.
pub const STACK_ALIGN: usize = 16;

/// Baud rate used for the boot console UART.
pub const BOOT_BAUD_RATE: u32 = 115200;

// ---------------------------------------------------------------------------
// FDT (Flattened Device Tree) structures and parsing
// ---------------------------------------------------------------------------

/// Magic value that must appear at the start of a valid DTB.
pub const FDT_MAGIC: u32 = 0xD00DFEED;

/// Expected FDT version (version 17 is standard for modern DTBs).
pub const FDT_VERSION: u16 = 17;

/// A parsed FDT header containing the essential fields needed during boot.
///
/// The FDT header occupies the first 40 bytes of the DTB blob. We extract
/// the fields that matter for early boot: total size, structure offset and
/// size, strings offset and size, and version.
///
/// The binary layout of the header (all fields big-endian `u32`):
///
/// | Offset | Field                | Our field          |
/// |--------|----------------------|--------------------|
/// | 0x00   | `magic`              | `magic`            |
/// | 0x04   | `totalsize`          | `totalsize`        |
/// | 0x08   | `off_dt_struct`      | `off_dt_struct`    |
/// | 0x0C   | `off_dt_strings`     | `off_dt_strings`   |
/// | 0x10   | `off_mem_rsvmap`     | *(not stored)*     |
/// | 0x14   | `version`            | `version`          |
/// | 0x18   | `last_comp_version`  | *(not stored)*     |
/// | 0x1C   | `boot_cpuid_phys`   | *(not stored)*     |
/// | 0x20   | `size_dt_strings`    | `size_dt_strings`  |
/// | 0x24   | `size_dt_struct`     | `size_dt_struct`   |
#[derive(Debug, Clone, Copy)]
pub struct FdtHeader {
    /// Magic number — must equal [`FDT_MAGIC`].
    pub magic: u32,
    /// Total size of the DTB blob in bytes.
    pub totalsize: u32,
    /// Offset from blob start to the structure block.
    pub off_dt_struct: u32,
    /// Size of the structure block in bytes.
    pub size_dt_struct: u32,
    /// Offset from blob start to the strings block.
    pub off_dt_strings: u32,
    /// Size of the strings block in bytes.
    pub size_dt_strings: u32,
    /// FDT version.
    pub version: u32,
}

impl FdtHeader {
    /// Parses an [`FdtHeader`] from a raw DTB pointer.
    ///
    /// # Safety
    ///
    /// `dtb_ptr` must point to at least 40 bytes of valid memory containing
    /// a well-formed FDT header. The pointer must be 4-byte aligned.
    pub unsafe fn from_raw(dtb_ptr: *const u8) -> Option<Self> {
        // SAFETY: Caller guarantees dtb_ptr points to at least 40 valid,
        // 4-byte-aligned bytes.
        let words = unsafe { core::slice::from_raw_parts(dtb_ptr as *const u32, 10) };
        Self::from_words(words)
    }

    /// Parses an [`FdtHeader`] from a byte slice.
    ///
    /// Returns `None` if the slice is too short or the magic number is wrong.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 40 {
            return None;
        }
        let words: [u32; 10] = core::array::from_fn(|i| {
            u32::from_be_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
        });
        Self::from_words(&words)
    }

    /// Shared parsing logic from a slice of 10 big-endian u32 words.
    fn from_words(words: &[u32]) -> Option<Self> {
        debug_assert!(words.len() >= 10);
        let magic = u32::from_be(words[0]);
        if magic != FDT_MAGIC {
            return None;
        }
        Some(Self {
            magic,
            totalsize: u32::from_be(words[1]),
            off_dt_struct: u32::from_be(words[2]),
            off_dt_strings: u32::from_be(words[3]),
            // words[4] = off_mem_rsvmap (not stored)
            version: u32::from_be(words[5]),
            // words[6] = last_comp_version (not stored)
            // words[7] = boot_cpuid_phys (not stored)
            size_dt_strings: u32::from_be(words[8]),
            size_dt_struct: u32::from_be(words[9]),
        })
    }

    /// Validates that the header fields are internally consistent.
    pub fn is_valid(&self) -> bool {
        self.magic == FDT_MAGIC
            && self.totalsize > 0
            && self.off_dt_struct < self.totalsize
            && self.off_dt_strings < self.totalsize
            && self.size_dt_struct + self.off_dt_struct <= self.totalsize
            && self.size_dt_strings + self.off_dt_strings <= self.totalsize
    }
}

/// Parsed boot information extracted from the DTB.
///
/// This struct is populated during [`boot_main`] and provides essential
/// hardware parameters to the rest of the kernel.
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    /// Physical address of the DTB blob.
    pub dtb_addr: usize,
    /// Parsed FDT header (if the DTB was valid).
    pub fdt: Option<FdtHeader>,
    /// ID of the core that is executing boot_main (always 0).
    pub boot_core: CoreId,
}

// ---------------------------------------------------------------------------
// BSS section zeroing
// ---------------------------------------------------------------------------

/// Zeroes the BSS section.
///
/// On bare-metal targets the BSS section is not automatically initialised
/// by any runtime loader. This function must be called before any static
/// mutable data is read.
///
/// # Safety
///
/// The caller must ensure that `bss_start` and `bss_end` are valid,
/// aligned pointers delimiting the BSS section, and that
/// `bss_start <= bss_end`.
pub unsafe fn zero_bss(bss_start: *mut u8, bss_end: *mut u8) {
    let mut ptr = bss_start;
    while ptr < bss_end {
        // SAFETY: Caller guarantees [bss_start, bss_end) is valid, writable,
        // and properly aligned. We write one byte at a time for simplicity.
        unsafe {
            ptr.write_volatile(0);
        }
        ptr = ptr.add(1);
    }
    // Ensure all writes are visible before we return.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// Returns the boundaries of the BSS section from the linker symbols.
///
/// # Safety
///
/// Must only be called after the program is loaded and the linker symbols
/// are valid (always true in a bare-metal binary).
pub unsafe fn bss_boundaries() -> (*mut u8, *mut u8) {
    // SAFETY: The linker guarantees __bss_start and __bss_end are valid
    // addresses delimiting the BSS section.
    let start = unsafe { core::ptr::addr_of!(__bss_start) as *mut u8 };
    let end = unsafe { core::ptr::addr_of!(__bss_end) as *mut u8 };
    (start, end)
}

// ---------------------------------------------------------------------------
// Core parking (secondary cores)
// ---------------------------------------------------------------------------

/// Parks the calling core in an infinite `WFE` loop.
///
/// Secondary cores (1–3) enter this function during boot and remain here
/// until core 0 writes to their spin-table entry and issues `SEV`.
#[inline(always)]
pub fn park_core() -> ! {
    loop {
        // SAFETY: WFE is a hint instruction that is always safe to execute.
        // It puts the core in a low-power state until an event or interrupt.
        core::arch::asm!("wfe", options(nostack, preserves_flags));
    }
}

// ---------------------------------------------------------------------------
// Exception vector table (naked assembly)
// ---------------------------------------------------------------------------

/// ARM64 exception vector table.
///
/// The AArch64 vector table consists of 16 entries, each 128 bytes
/// (0x80) apart. The layout is:
///
/// | Offset | Entry                              | Description                       |
/// |--------|------------------------------------|-----------------------------------|
/// | 0x000  | `sync_el1_sp0`                     | Synchronous exception at EL1, SP0 |
/// | 0x080  | `irq_el1_sp0`                      | IRQ at EL1, SP0                   |
/// | 0x100  | `fiq_el1_sp0`                      | FIQ at EL1, SP0                   |
/// | 0x180  | `serror_el1_sp0`                   | SError at EL1, SP0                |
/// | 0x200  | `sync_el1_spx`                     | Synchronous exception at EL1, SPx |
/// | 0x280  | `irq_el1_spx`                      | IRQ at EL1, SPx                   |
/// | 0x300  | `fiq_el1_spx`                      | FIQ at EL1, SPx                   |
/// | 0x380  | `serror_el1_spx`                   | SError at EL1, SPx                |
/// | 0x400  | `sync_el0_aarch64`                 | Sync from lower EL (AArch64)      |
/// | 0x480  | `irq_el0_aarch64`                  | IRQ from lower EL (AArch64)       |
/// | 0x500  | `fiq_el0_aarch64`                  | FIQ from lower EL (AArch64)       |
/// | 0x580  | `serror_el0_aarch64`               | SError from lower EL (AArch64)    |
/// | 0x600  | `sync_el0_aarch32`                 | Sync from lower EL (AArch32)      |
/// | 0x680  | `irq_el0_aarch32`                  | IRQ from lower EL (AArch32)       |
/// | 0x700  | `fiq_el0_aarch32`                  | FIQ from lower EL (AArch32)       |
/// | 0x780  | `serror_el0_aarch32`               | SError from lower EL (AArch32)    |
///
/// Each handler currently parks the core in an infinite loop. In a full
/// kernel these would save context and dispatch to proper handlers.
#[naked]
pub unsafe extern "C" fn exception_vector_table() {
    // SAFETY: This is a naked function — the entire body is inline assembly
    // that implements the ARM64 exception vector table layout. Each entry
    // is padded to exactly 128 bytes (0x80).
    core::arch::asm!(
        // --- Current EL with SP0 ---
        "b {sync_el1_sp0}",
        ".align 7",   // 0x080
        "b {irq_el1_sp0}",
        ".align 7",   // 0x100
        "b {fiq_el1_sp0}",
        ".align 7",   // 0x180
        "b {serror_el1_sp0}",
        ".align 7",   // 0x200
        // --- Current EL with SPx ---
        "b {sync_el1_spx}",
        ".align 7",   // 0x280
        "b {irq_el1_spx}",
        ".align 7",   // 0x300
        "b {fiq_el1_spx}",
        ".align 7",   // 0x380
        "b {serror_el1_spx}",
        ".align 7",   // 0x400
        // --- Lower EL, AArch64 ---
        "b {sync_el0_aarch64}",
        ".align 7",   // 0x480
        "b {irq_el0_aarch64}",
        ".align 7",   // 0x500
        "b {fiq_el0_aarch64}",
        ".align 7",   // 0x580
        "b {serror_el0_aarch64}",
        ".align 7",   // 0x600
        // --- Lower EL, AArch32 ---
        "b {sync_el0_aarch32}",
        ".align 7",   // 0x680
        "b {irq_el0_aarch32}",
        ".align 7",   // 0x700
        "b {fiq_el0_aarch32}",
        ".align 7",   // 0x780
        "b {serror_el0_aarch32}",
        ".align 7",   // 0x800
        sync_el1_sp0      = sym sync_el1_sp0_handler,
        irq_el1_sp0       = sym irq_el1_sp0_handler,
        fiq_el1_sp0       = sym fiq_el1_sp0_handler,
        serror_el1_sp0    = sym serror_el1_sp0_handler,
        sync_el1_spx      = sym sync_el1_spx_handler,
        irq_el1_spx       = sym irq_el1_spx_handler,
        fiq_el1_spx       = sym fiq_el1_spx_handler,
        serror_el1_spx    = sym serror_el1_spx_handler,
        sync_el0_aarch64  = sym sync_el0_aarch64_handler,
        irq_el0_aarch64   = sym irq_el0_aarch64_handler,
        fiq_el0_aarch64   = sym fiq_el0_aarch64_handler,
        serror_el0_aarch64 = sym serror_el0_aarch64_handler,
        sync_el0_aarch32  = sym sync_el0_aarch32_handler,
        irq_el0_aarch32   = sym irq_el0_aarch32_handler,
        fiq_el0_aarch32   = sym fiq_el0_aarch32_handler,
        serror_el0_aarch32 = sym serror_el0_aarch32_handler,
        options(noreturn)
    );
}

// ---------------------------------------------------------------------------
// Individual exception handler stubs
// ---------------------------------------------------------------------------

/// Handler for synchronous exceptions at EL1 using SP0.
fn sync_el1_sp0_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for IRQ exceptions at EL1 using SP0.
fn irq_el1_sp0_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for FIQ exceptions at EL1 using SP0.
fn fiq_el1_sp0_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for SError exceptions at EL1 using SP0.
fn serror_el1_sp0_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for synchronous exceptions at EL1 using SPx.
fn sync_el1_spx_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for IRQ exceptions at EL1 using SPx.
fn irq_el1_spx_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for FIQ exceptions at EL1 using SPx.
fn fiq_el1_spx_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for SError exceptions at EL1 using SPx.
fn serror_el1_spx_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for synchronous exceptions from lower EL (AArch64).
fn sync_el0_aarch64_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for IRQ exceptions from lower EL (AArch64).
fn irq_el0_aarch64_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for FIQ exceptions from lower EL (AArch64).
fn fiq_el0_aarch64_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for SError exceptions from lower EL (AArch64).
fn serror_el0_aarch64_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for synchronous exceptions from lower EL (AArch32).
fn sync_el0_aarch32_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for IRQ exceptions from lower EL (AArch32).
fn irq_el0_aarch32_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for FIQ exceptions from lower EL (AArch32).
fn fiq_el0_aarch32_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Handler for SError exceptions from lower EL (AArch32).
fn serror_el0_aarch32_handler() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// VBAR_EL1 installation
// ---------------------------------------------------------------------------

/// Installs the exception vector table by writing its address to `VBAR_EL1`.
///
/// # Safety
///
/// Must be called only from EL1. The table must be aligned to 2 KiB
/// (0x800) as required by the architecture.
#[inline(always)]
pub unsafe fn install_exception_vector_table() {
    // SAFETY: The caller guarantees we are executing at EL1 and the
    // exception_vector_table function is properly aligned and contains
    // a valid vector table.
    core::arch::asm!(
        "adr x0, {table}",
        "msr vbar_el1, x0",
        "isb",
        table = sym exception_vector_table,
        out("x0") _,
        options(nostack, preserves_flags)
    );
}

// ---------------------------------------------------------------------------
// _start — entry point (naked assembly)
// ---------------------------------------------------------------------------

/// Raw entry point invoked by the Pi 5 bootloader at physical address
/// [`KERNEL_ENTRY`] (`0x80000`).
///
/// On entry the register state is:
///
/// * `x0` — physical address of the DTB / FDT.
/// * `x1`–`x3` — zero (reserved).
///
/// This function performs the following steps:
///
/// 1. Saves `x0` (DTB pointer) into `x6` before any register is clobbered.
/// 2. Reads the core ID from `MPIDR_EL1`.
/// 3. Parks secondary cores (1–3) in a `WFE` loop.
/// 4. Sets up the stack pointer for core 0 (16 KiB, 16-byte aligned).
/// 5. Zeros the BSS section.
/// 6. Installs the exception vector table.
/// 7. Jumps to [`boot_main`] with the DTB pointer in `x0`.
#[naked]
#[link_section = ".text.boot"]
pub unsafe extern "C" fn _start() {
    // SAFETY: Naked function — the entire body is hand-written assembly
    // that implements the early boot sequence.
    core::arch::asm!(
        // ---- Preserve the DTB pointer (x0) before we clobber anything ----
        "mov     x6, x0",

        // ---- Read the core ID from MPIDR_EL1 (Aff0 field) ----
        "mrs     x5, mpidr_el1",
        "and     x5, x5, #0xFF",        // core_id = MPIDR_EL1.Aff0

        // ---- Park secondary cores (1-3) ----
        "cbz     x5, 2f",               // core 0 continues, others fall through

        // Secondary core parking loop
        "1:",
        "wfe",
        "b       1b",

        // ---- Core 0: set up the stack pointer ----
        "2:",
        // Place the stack above the BSS end.
        // SP = __bss_end + STACK_SIZE_PER_CORE, aligned to 16 bytes.
        "adrp    x2, __bss_end",
        "add     x2, x2, #:lo12:__bss_end",
        "add     x2, x2, {stack_size}",

        // Align SP to 16 bytes (AAPCS requirement).
        "and     x2, x2, #{stack_align_mask}",
        "mov     sp, x2",

        // ---- Zero the BSS section ----
        "adrp    x3, __bss_start",
        "add     x3, x3, #:lo12:__bss_start",
        "adrp    x4, __bss_end",
        "add     x4, x4, #:lo12:__bss_end",

        "3:",
        "cmp     x3, x4",
        "b.ge    4f",
        "str     xzr, [x3], #8",
        "b       3b",
        "4:",

        // ---- Install the exception vector table ----
        "adr     x0, {vec_table}",
        "msr     vbar_el1, x0",
        "isb",

        // ---- Restore the DTB pointer and jump to boot_main ----
        "mov     x0, x6",               // x0 = saved DTB pointer
        "b       {boot_main}",

        stack_size = const STACK_SIZE_PER_CORE,
        stack_align_mask = const !(STACK_ALIGN - 1),
        vec_table = sym exception_vector_table,
        boot_main = sym boot_main,
        options(noreturn)
    );
}

// ---------------------------------------------------------------------------
// boot_main — high-level entry point
// ---------------------------------------------------------------------------

/// Global storing the DTB physical address passed by the bootloader.
static mut DTB_ADDRESS: usize = 0;

/// High-level entry point called after assembly boot setup is complete.
///
/// This function runs only on core 0. It:
///
/// 1. Saves the DTB address from the bootloader.
/// 2. Initialises the UART for debug output.
/// 3. Parses the FDT header.
/// 4. Constructs a [`BootInfo`] and calls the platform-specific main.
///
/// Cores 1–3 never reach this function — they are parked in a `WFE` loop
/// by the [`_start`] assembly entry point.
///
/// # Safety
///
/// This function must be called only from EL1, after the stack pointer has
/// been set up, BSS zeroed, and the exception vector table installed.
/// The `dtb_ptr` argument must be either 0 or a valid physical address
/// pointing to a DTB blob.
pub unsafe fn boot_main(dtb_ptr: usize) -> ! {
    // Save the DTB address.
    // SAFETY: We are single-threaded at this point (core 0 only, secondary
    // cores are still parked).
    unsafe {
        DTB_ADDRESS = dtb_ptr;
    }

    // Initialise the UART for early debug output.
    let uart_base = PERIPHERAL_BASE + UART_BASE_OFFSET;
    let uart = Uart::new(uart_base);
    uart.init(BOOT_BAUD_RATE);
    uart.write_str("VUMA Pi 5 boot\n");

    // Parse the FDT header from the DTB passed by the bootloader.
    let fdt = if dtb_ptr != 0 {
        // SAFETY: The bootloader guarantees that dtb_ptr points to a valid
        // DTB blob if it is non-zero.
        unsafe { FdtHeader::from_raw(dtb_ptr as *const u8) }
    } else {
        None
    };

    if let Some(ref hdr) = fdt {
        if hdr.is_valid() {
            uart.write_str("FDT parsed OK\n");
        } else {
            uart.write_str("FDT header invalid\n");
        }
    } else {
        uart.write_str("No FDT found\n");
    }

    // Build BootInfo for the rest of the kernel.
    let boot_info = BootInfo {
        dtb_addr: dtb_ptr,
        fdt,
        boot_core: CoreId::CORE0,
    };

    // Call the user-supplied main function.
    main(&boot_info);

    // If main() returns, park core 0.
    uart.write_str("main() returned — parking core 0\n");
    park_core()
}

/// User-provided entry point.
///
/// Override this function with the application's main logic. It receives a
/// reference to [`BootInfo`] containing hardware details parsed from the DTB.
///
/// The default implementation simply returns immediately.
#[allow(clippy::module_name_repetitions)]
pub fn main(_boot_info: &BootInfo) {
    // Default: do nothing. The user should replace this.
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- FdtHeader tests ---

    #[test]
    fn fdt_header_from_bytes_valid_magic() {
        // Construct a minimal valid FDT header (40 bytes).
        let mut bytes = [0u8; 40];
        // [0] magic = 0xD00DFEED
        bytes[0..4].copy_from_slice(&0xD00DFEEDu32.to_be_bytes());
        // [1] totalsize = 1024
        bytes[4..8].copy_from_slice(&1024u32.to_be_bytes());
        // [2] off_dt_struct = 48
        bytes[8..12].copy_from_slice(&48u32.to_be_bytes());
        // [3] off_dt_strings = 800
        bytes[12..16].copy_from_slice(&800u32.to_be_bytes());
        // [4] off_mem_rsvmap = 56 (not parsed)
        bytes[16..20].copy_from_slice(&56u32.to_be_bytes());
        // [5] version = 17
        bytes[20..24].copy_from_slice(&17u32.to_be_bytes());
        // [6] last_comp_version = 16 (not parsed)
        bytes[24..28].copy_from_slice(&16u32.to_be_bytes());
        // [7] boot_cpuid_phys = 0 (not parsed)
        bytes[28..32].copy_from_slice(&0u32.to_be_bytes());
        // [8] size_dt_strings = 100
        bytes[32..36].copy_from_slice(&100u32.to_be_bytes());
        // [9] size_dt_struct = 700
        bytes[36..40].copy_from_slice(&700u32.to_be_bytes());

        let hdr = FdtHeader::from_bytes(&bytes).expect("should parse valid header");
        assert_eq!(hdr.magic, FDT_MAGIC);
        assert_eq!(hdr.totalsize, 1024);
        assert_eq!(hdr.off_dt_struct, 48);
        assert_eq!(hdr.off_dt_strings, 800);
        assert_eq!(hdr.version, 17);
        assert_eq!(hdr.size_dt_strings, 100);
        assert_eq!(hdr.size_dt_struct, 700);
    }

    #[test]
    fn fdt_header_from_bytes_invalid_magic_returns_none() {
        let mut bytes = [0u8; 40];
        // Invalid magic
        bytes[0..4].copy_from_slice(&0xDEADBEEFu32.to_be_bytes());
        assert!(FdtHeader::from_bytes(&bytes).is_none());
    }

    #[test]
    fn fdt_header_from_bytes_short_slice_returns_none() {
        let bytes = [0u8; 20]; // Too short for a full header
        assert!(FdtHeader::from_bytes(&bytes).is_none());
    }

    #[test]
    fn fdt_header_is_valid_rejects_inconsistent_offsets() {
        let hdr = FdtHeader {
            magic: FDT_MAGIC,
            totalsize: 100,
            off_dt_struct: 200, // Past totalsize — invalid
            off_dt_strings: 10,
            size_dt_struct: 50,
            size_dt_strings: 50,
            version: 17,
        };
        assert!(!hdr.is_valid());
    }

    #[test]
    fn fdt_header_is_valid_accepts_consistent_header() {
        let hdr = FdtHeader {
            magic: FDT_MAGIC,
            totalsize: 1024,
            off_dt_struct: 48,
            off_dt_strings: 800,
            size_dt_struct: 700,
            size_dt_strings: 100,
            version: 17,
        };
        assert!(hdr.is_valid());
    }

    #[test]
    fn boot_constants_are_correct() {
        assert_eq!(KERNEL_ENTRY, 0x80_000);
        assert_eq!(STACK_SIZE_PER_CORE, 16 * 1024);
        assert_eq!(STACK_ALIGN, 16);
        assert_eq!(FDT_MAGIC, 0xD00DFEED);
        assert_eq!(BOOT_BAUD_RATE, 115200);
    }

    #[test]
    fn zero_bss_clears_memory() {
        let mut buf = [0xAAu8; 64];
        // SAFETY: buf is a valid, aligned, local buffer.
        unsafe {
            zero_bss(buf.as_mut_ptr(), buf.as_mut_ptr().add(buf.len()));
        }
        assert!(buf.iter().all(|&b| b == 0), "BSS region should be zeroed");
    }
}
