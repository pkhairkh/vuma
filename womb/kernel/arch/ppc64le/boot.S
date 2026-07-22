/* boot.S — ppc64le bare-metal boot entry (QEMU pSeries)
 * QEMU: qemu-system-ppc64le -M pseries -kernel kernel.bin -serial stdio
 * Entry: QEMU loads kernel at 0x400000. r3 = dtb pointer.
 */
    .section .text
    .global _start
_start:
    lis sp, 0x8000
    ori sp, sp, 0            /* sp = 0x80000000 */
    /* Zero BSS */
    lis r4, bss_start@h
    ori r4, r4, bss_start@l
    lis r5, bss_end@h
    ori r5, r5, bss_end@l
    li r6, 0
1:
    cmpd r4, r5
    bge 2f
    std r6, 0(r4)
    addi r4, r4, 8
    b 1b
2:
    mr r31, r3              /* save dtb pointer */
    bl main
3:
    sync
    rfid
    b 3b

    .section .bss
    .align 16
bss_start:
    .skip 65536
bss_end:
