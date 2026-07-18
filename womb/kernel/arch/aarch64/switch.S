/* switch.S — aarch64 context switch
 *
 * arch_load_context: loads registers from the superset TrapFrame.
 * x0 = pointer to TrapFrame (AArch64 ABI first arg).
 *
 * TrapFrame layout (same as x86_64 but with aarch64 register names):
 *   gpr[0]  (offset 0)   = x0
 *   gpr[1]  (offset 8)   = x1
 *   ...
 *   gpr[30] (offset 240) = x30 (lr)
 *   pc      (offset 256) = elr_el1
 *   status  (offset 264) = spsr_el1
 *   sp      (offset 288) = sp_el0
 */

    .section .text
    .global arch_load_context

arch_load_context:
    /* x0 = pointer to TrapFrame */

    /* Save x0 (TrapFrame pointer) in x1 — we'll restore x0 from TrapFrame last */
    mov x1, x0

    /* Load sp from offset 288 */
    ldr x2, [x1, #288]
    msr sp_el0, x2

    /* Load spsr_el1 from offset 264 */
    ldr x2, [x1, #264]
    msr spsr_el1, x2

    /* Load elr_el1 (return address) from offset 256 */
    ldr x2, [x1, #256]
    msr elr_el1, x2

    /* Load GPRs x2-x30 from TrapFrame */
    ldp x2, x3, [x1, #16]
    ldp x4, x5, [x1, #32]
    ldp x6, x7, [x1, #48]
    ldp x8, x9, [x1, #64]
    ldp x10, x11, [x1, #80]
    ldp x12, x13, [x1, #96]
    ldp x14, x15, [x1, #112]
    ldp x16, x17, [x1, #128]
    ldp x18, x19, [x1, #144]
    ldp x20, x21, [x1, #160]
    ldp x22, x23, [x1, #176]
    ldp x24, x25, [x1, #192]
    ldp x26, x27, [x1, #208]
    ldp x28, x29, [x1, #224]
    ldr x30, [x1, #240]

    /* Load x0 from offset 0 — must be last */
    ldr x0, [x1, #0]

    /* eret: loads elr_el1 into pc, spsr_el1 into pstate */
    eret
