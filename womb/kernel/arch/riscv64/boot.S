/* boot.S — riscv64 bare-metal boot entry (QEMU virt)
 * QEMU: qemu-system-riscv64 -M virt -kernel kernel.bin -serial stdio
 * Entry: QEMU loads kernel at 0x80200000. a0=hartid, a1=dtb pointer.
 */
    .section .text
    .global _start
_start:
    la sp, 0x81000000       /* stack top */
    /* Zero BSS */
    la t0, bss_start
    la t1, bss_end
1:
    bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:
    /* Save dtb pointer */
    mv s10, a0              /* s10 = hartid */
    mv s11, a1              /* s11 = dtb_addr */
    /* Call VUMA main() */
    call main
3:
    wfi
    j 3b

    .section .bss
    .align 16
bss_start:
    .skip 65536
bss_end:
