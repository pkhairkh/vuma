/* ---------------------------------------------------------------------------
 * link.ld — ARM64 linker script for Raspberry Pi 5 bare-metal
 *
 * Target: BCM2712 (4 × Cortex-A76), loaded by VideoCore at 0x80000.
 *
 * Memory layout
 *   RAM  : 0x0008_0000 – 0x0088_0000  (8 MiB, kernel image + data + stacks)
 *   MMIO : 0x0010_0000 – 0x0020_0000  (1 MiB, memory-mapped I/O window)
 *
 * Sections are aligned to 4 KiB page boundaries to simplify any future
 * MPU / page-table mappings.
 *
 * Stack: 64 KiB per core × 4 cores = 256 KiB, placed at the end of RAM.
 * --------------------------------------------------------------------------- */

ENTRY(_start)

MEMORY
{
    RAM  (rwx) : ORIGIN = 0x80000,    LENGTH = 8M
    MMIO (rw)  : ORIGIN = 0x100000,   LENGTH = 1M
}

/* 4 KiB page alignment macro */
PAGE_ALIGN = ALIGN(4096);

SECTIONS
{
    /* ---- Boot code: must be the very first section at the kernel load addr */
    .text.boot : {
        *(.text.boot)
    } > RAM

    . = PAGE_ALIGN;

    /* ---- Main code */
    .text : {
        *(.text .text.*)
    } > RAM

    . = PAGE_ALIGN;

    /* ---- Read-only data */
    .rodata : {
        *(.rodata .rodata.*)
    } > RAM

    . = PAGE_ALIGN;

    /* ---- Initialised data */
    .data : {
        *(.data .data.*)
    } > RAM

    . = PAGE_ALIGN;

    /* ---- Zero-initialised data */
    .bss (NOLOAD) : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4096);
        __bss_end = .;
    } > RAM

    . = PAGE_ALIGN;

    /* ---- Per-core stacks (64 KiB × 4 cores = 256 KiB) ---- */
    __stack_start = .;

    /*  Core 0  */
    . = . + 65536;
    __stack_core0 = .;

    /*  Core 1  */
    . = . + 65536;
    __stack_core1 = .;

    /*  Core 2  */
    . = . + 65536;
    __stack_core2 = .;

    /*  Core 3  */
    . = . + 65536;
    __stack_core3 = .;

    __stack_end = .;

    . = ALIGN(4096);

    /* ---- End of kernel image ---- */
    __kernel_end = .;

    /* ---- Discard sections not needed on bare-metal ---- */
    /DISCARD/ : {
        *(.comment)
        *(.note.*)
        *(.eh_frame*)
        *(.ARM.exidx*)
    }
}
