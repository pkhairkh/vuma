ENTRY(_start)
SECTIONS {
    . = 0x400000;
    .text : { *(.text) *(.text.*) }
    .rodata : { *(.rodata) *(.rodata.*) }
    .data : { *(.data) *(.data.*) }
    .bss : { bss_start = .; *(.bss) *(.bss.*) *(COMMON); bss_end = .; }
    /DISCARD/ : { *(.comment) *(.note.*) *(.eh_frame) }
}
