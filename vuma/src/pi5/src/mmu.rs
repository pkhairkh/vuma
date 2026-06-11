//! MMU and page table setup for BCM2712 (Raspberry Pi 5).
//!
//! Provides identity-mapped page tables for bare-metal execution,
//! configuring the MMU for proper memory access.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Page size (4KB on AArch64).
pub const PAGE_SIZE: usize = 4096;
/// Page shift.
pub const PAGE_SHIFT: usize = 12;

/// Page table entry flags.
#[allow(non_upper_case_globals)]
pub mod flags {
    /// Entry is valid.
    pub const VALID: u64 = 1 << 0;
    /// Entry is a page (not a block).
    pub const PAGE: u64 = 1 << 1;
    /// Access flag (AF) — accessed.
    pub const ACCESSED: u64 = 1 << 10;
    /// Normal memory (Inner Shareable).
    pub const INNER_SHAREABLE: u64 = 3 << 8;
    /// Read-write at EL1.
    pub const RW_EL1: u64 = 0;
    /// Read-only at EL1.
    pub const RO_EL1: u64 = 1 << 7; // AP[2]
    /// Device memory (nGnRnE).
    pub const DEVICE_nGnRnE: u64 = 0;
    /// Normal non-cacheable.
    pub const NORMAL_NC: u64 = 4 << 2;
    /// Normal cacheable.
    pub const NORMAL_CACHEABLE: u64 = 7 << 2; // Inner: WB RW-Allocate, Outer: Same
}

/// Memory region descriptor for initial mapping.
#[derive(Debug, Clone)]
pub struct MemoryRegion {
    /// Physical start address.
    pub phys_start: u64,
    /// Virtual start address (identity-mapped = phys_start).
    pub virt_start: u64,
    /// Size in bytes.
    pub size: u64,
    /// Page table entry flags.
    pub flags: u64,
    /// Description for debugging.
    pub description: &'static str,
}

/// Default memory map for BCM2712.
pub fn bcm2712_default_regions() -> Vec<MemoryRegion> {
    vec![
        // Peripheral IO (1 MB at 0x1C000000 - GIC, GPIO, UART, etc.)
        MemoryRegion {
            phys_start: 0x1C00_0000,
            virt_start: 0x1C00_0000,
            size: 0x0020_0000,
            flags: flags::VALID
                | flags::PAGE
                | flags::ACCESSED
                | flags::DEVICE_nGnRnE
                | flags::RW_EL1,
            description: "BCM2712 Peripherals",
        },
        // RAM (first 1GB identity-mapped, cacheable)
        MemoryRegion {
            phys_start: 0x0000_0000,
            virt_start: 0x0000_0000,
            size: 0x4000_0000,
            flags: flags::VALID
                | flags::PAGE
                | flags::ACCESSED
                | flags::NORMAL_CACHEABLE
                | flags::INNER_SHAREABLE
                | flags::RW_EL1,
            description: "DRAM (first 1GB)",
        },
        // MMIO (PCIe, XHCI at 0x1000_0000)
        MemoryRegion {
            phys_start: 0x1000_0000,
            virt_start: 0x1000_0000,
            size: 0x0010_0000,
            flags: flags::VALID
                | flags::PAGE
                | flags::ACCESSED
                | flags::DEVICE_nGnRnE
                | flags::RW_EL1,
            description: "BCM2712 MMIO",
        },
    ]
}

/// A 4-level page table (4096 entries of u64).
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [u64; 512],
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PageTable {
    pub const fn new() -> Self {
        Self { entries: [0; 512] }
    }

    /// Map a single page in the page table.
    pub fn map_page(&mut self, virt_addr: u64, phys_addr: u64, flags: u64) {
        // Simple 1:1 mapping for L0 table
        let index = ((virt_addr >> (PAGE_SHIFT + 9 * 3)) & 0x1FF) as usize;
        self.entries[index] = phys_addr | flags;
    }
}

/// Initialize the MMU with identity-mapped page tables.
///
/// # Safety
/// Must be called only once, before enabling the MMU.
#[cfg(target_arch = "aarch64")]
pub unsafe fn init_mmu(l0_table: &mut PageTable) {
    // Map all regions
    for region in bcm2712_default_regions() {
        let mut offset = 0u64;
        while offset < region.size {
            let vaddr = region.virt_start + offset;
            let paddr = region.phys_start + offset;
            l0_table.map_page(vaddr, paddr, region.flags);
            offset += PAGE_SIZE as u64;
        }
    }

    // Set MAIR_EL1 (Memory Attribute Indirection Register)
    // Attr0: Device nGnRnE
    // Attr1: Normal Non-Cacheable
    // Attr2: Normal Cacheable (WB RW-Allocate)
    core::arch::asm!("msr mair_el1, {0}", in(reg) 0x00FF0444_u64);

    // Set TCR_EL1 (Translation Control Register)
    // 4KB granule, Inner/Outer shareable, 48-bit VA/PA
    core::arch::asm!("msr tcr_el1, {0}", in(reg) 0x0000_3519_u64);

    // Set TTBR0_EL1 (Translation Table Base Register)
    core::arch::asm!("msr ttbr0_el1, {0}", in(reg) l0_table as *const _ as u64);

    // Invalidate TLB
    core::arch::asm!("tlbi vmalle1is");

    // Enable MMU and caches in SCTLR_EL1
    let mut sctlr: u64;
    core::arch::asm!("mrs {0}, sctlr_el1", out(reg) sctlr);
    sctlr |= (1 << 0) | // M: MMU enable
              (1 << 2) | // C: Data cache
              (1 << 12); // I: Instruction cache
    core::arch::asm!("msr sctlr_el1, {0}", in(reg) sctlr);

    // Data synchronization barrier
    core::arch::asm!("dsb ish");
    core::arch::asm!("isb");
}

/// Initialize the MMU with identity-mapped page tables.
///
/// Stub for non-AArch64 targets — does nothing.
///
/// # Safety
/// Must be called only once, before enabling the MMU.
#[cfg(not(target_arch = "aarch64"))]
pub unsafe fn init_mmu(l0_table: &mut PageTable) {
    // On non-AArch64 targets, just populate the page table entries
    // without touching system registers.
    for region in bcm2712_default_regions() {
        let mut offset = 0u64;
        while offset < region.size {
            let vaddr = region.virt_start + offset;
            let paddr = region.phys_start + offset;
            l0_table.map_page(vaddr, paddr, region.flags);
            offset += PAGE_SIZE as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_table_new() {
        let table = PageTable::new();
        // All entries should be zero
        for (i, &entry) in table.entries.iter().enumerate() {
            assert_eq!(entry, 0, "Entry {} should be zero, got {:#x}", i, entry);
        }
    }

    #[test]
    fn test_map_page() {
        let mut table = PageTable::new();

        // Map address 0x0000_0000 (index 0 in L0)
        table.map_page(0x0000_0000, 0x0000_0000, flags::VALID | flags::PAGE);
        let index0 = ((0x0000_0000u64 >> (PAGE_SHIFT + 9 * 3)) & 0x1FF) as usize;
        assert_ne!(
            table.entries[index0], 0,
            "Entry at index {} should be non-zero",
            index0
        );
        assert_eq!(
            table.entries[index0] & flags::VALID,
            flags::VALID,
            "Entry should have VALID flag set"
        );
        assert_eq!(
            table.entries[index0] & flags::PAGE,
            flags::PAGE,
            "Entry should have PAGE flag set"
        );

        // Map a higher address (0x1C00_0000 — peripherals)
        let periph_flags = flags::VALID | flags::PAGE | flags::ACCESSED | flags::DEVICE_nGnRnE;
        table.map_page(0x1C00_0000, 0x1C00_0000, periph_flags);
        let index1c = ((0x1C00_0000u64 >> (PAGE_SHIFT + 9 * 3)) & 0x1FF) as usize;
        assert_ne!(
            table.entries[index1c], 0,
            "Entry at index {} should be non-zero",
            index1c
        );
        assert_eq!(
            table.entries[index1c] & flags::ACCESSED,
            flags::ACCESSED,
            "Peripheral entry should have ACCESSED flag"
        );
    }

    #[test]
    fn test_bcm2712_default_regions() {
        let regions = bcm2712_default_regions();

        // Should have exactly 3 regions
        assert_eq!(regions.len(), 3, "Expected 3 default memory regions");

        // All regions should be identity-mapped
        for region in &regions {
            assert_eq!(
                region.phys_start, region.virt_start,
                "Region '{}' should be identity-mapped",
                region.description
            );
        }

        // Verify Peripherals region
        let periph = &regions[0];
        assert_eq!(periph.phys_start, 0x1C00_0000);
        assert_eq!(periph.size, 0x0020_0000);
        assert_eq!(periph.flags & flags::DEVICE_nGnRnE, flags::DEVICE_nGnRnE);

        // Verify DRAM region
        let dram = &regions[1];
        assert_eq!(dram.phys_start, 0x0000_0000);
        assert_eq!(dram.size, 0x4000_0000);
        assert_eq!(
            dram.flags & flags::NORMAL_CACHEABLE,
            flags::NORMAL_CACHEABLE
        );
        assert_eq!(dram.flags & flags::INNER_SHAREABLE, flags::INNER_SHAREABLE);

        // Verify MMIO region
        let mmio = &regions[2];
        assert_eq!(mmio.phys_start, 0x1000_0000);
        assert_eq!(mmio.size, 0x0010_0000);
        assert_eq!(mmio.flags & flags::DEVICE_nGnRnE, flags::DEVICE_nGnRnE);

        // All regions should have VALID, PAGE, ACCESSED flags
        for region in &regions {
            assert_ne!(
                region.flags & flags::VALID,
                0,
                "'{}' missing VALID",
                region.description
            );
            assert_ne!(
                region.flags & flags::PAGE,
                0,
                "'{}' missing PAGE",
                region.description
            );
            assert_ne!(
                region.flags & flags::ACCESSED,
                0,
                "'{}' missing ACCESSED",
                region.description
            );
        }
    }
}
