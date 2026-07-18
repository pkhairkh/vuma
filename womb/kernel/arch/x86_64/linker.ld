/* linker.ld — x86_64 bare-metal kernel linker script
 *
 * QEMU: qemu-system-x86_64 -kernel kernel.bin -serial stdio
 *
 * The kernel is loaded at 1 MB (0x100000) by the multiboot2 loader.
 * BSS is at 0x90000 (below the stack at 0x90000).
 */

ENTRY(_start)

SECTIONS {
    /* Kernel loaded at 1 MB by multiboot */
    . = 0x100000;

    /* Multiboot header must be in the first 8 KB */
    .multiboot : {
        *(.multiboot)
    }

    /* Code */
    .text : {
        *(.text)
        *(.text.*)
    }

    /* Read-only data */
    .rodata : {
        *(.rodata)
        *(.rodata.*)
    }

    /* Initialized data */
    .data : {
        *(.data)
        *(.data.*)
    }

    /* BSS (uninitialized data) */
    .bss : {
        bss_start = .;
        *(.bss)
        *(.bss.*)
        *(COMMON)
        bss_end = .;
    }

    /* Discard unneeded sections */
    /DISCARD/ : {
        *(.comment)
        *(.note.*)
        *(.eh_frame)
    }
}
