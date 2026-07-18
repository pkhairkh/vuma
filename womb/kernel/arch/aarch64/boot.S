/* boot.S — aarch64 bare-metal boot entry (QEMU virt)
 *
 * QEMU: qemu-system-aarch64 -M virt -cpu cortex-a57 -kernel kernel.bin -serial stdio
 *
 * Entry: QEMU loads the kernel at 0x40080000. x0 = dtb pointer.
 *
 * TrapFrame canonical GPR slot mapping (aarch64):
 *   slot 0  = x0  (TrapFrame.gpr offset 0)
 *   slot 1  = x1  (offset 8)
 *   ...
 *   slot 30 = x30 (offset 240)
 *   TrapFrame.pc     = elr_el1  (offset 256)
 *   TrapFrame.status = spsr_el1 (offset 264)
 *   TrapFrame.sp     = sp_el0   (offset 288)
 */

    .section .text
    .global _start
_start:
    /* Set up stack */
    ldr x1, =0x48000000
    mov sp, x1

    /* Zero BSS */
    ldr x1, =bss_start
    ldr x2, =bss_end
    mov x3, xzr
1:
    cmp x1, x2
    b.ge 2f
    str x3, [x1], #8
    b 1b
2:

    /* x0 still holds dtb pointer — save it */
    mov x20, x0

    /* Call VUMA main() */
    bl main

    /* Halt on return */
3:
    wfi
    b 3b

    .section .bss
    .align 16
bss_start:
    .skip 65536
bss_end:
