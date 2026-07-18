/* linker.ld — Hosted-mode linker script (Linux ELF)
 *
 * Uses the host's default ELF layout. This is a minimal script that
 * just sets the entry point and arranges standard sections.
 */

ENTRY(_start)

SECTIONS {
    . = 0x400000;

    .text : {
        *(.text)
        *(.text.*)
    }

    .rodata : {
        *(.rodata)
        *(.rodata.*)
    }

    .data : {
        *(.data)
        *(.data.*)
    }

    .bss : {
        bss_start = .;
        *(.bss)
        *(.bss.*)
        *(COMMON)
        bss_end = .;
    }

    /DISCARD/ : {
        *(.comment)
        *(.note.*)
        *(.eh_frame)
    }
}
