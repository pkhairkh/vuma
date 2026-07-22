/* trap.S — aarch64 trap entry/exit
 *
 * VBAR_EL1 vector table: 16 entries (4 exception types × 4 sync/irq/fiq/serror).
 * Each entry is 128 bytes (enough for a branch + save sequence).
 *
 * trap_entry saves x0-x30 into TrapFrame.gpr[0..30], saves sp_el0, elr_el1,
 * spsr_el1, esr_el1, calls trap_handler, then trap_exit restores + eret.
 */

    .section .text

    .macro VECTOR_ENTRY handler
    b \handler
    .align 7
    .endm

    .align 11
    .global vbar_el1_base
vbar_el1_base:
    /* Current EL with SP_EL0 */
    VECTOR_ENTRY trap_entry_sync
    VECTOR_ENTRY trap_entry_irq
    VECTOR_ENTRY trap_entry_fiq
    VECTOR_ENTRY trap_entry_serror

    /* Current EL with SP_ELx */
    VECTOR_ENTRY trap_entry_sync
    VECTOR_ENTRY trap_entry_irq
    VECTOR_ENTRY trap_entry_fiq
    VECTOR_ENTRY trap_entry_serror

    /* Lower EL (AArch64) */
    VECTOR_ENTRY trap_entry_sync
    VECTOR_ENTRY trap_entry_irq
    VECTOR_ENTRY trap_entry_fiq
    VECTOR_ENTRY trap_entry_serror

    /* Lower EL (AArch32) */
    VECTOR_ENTRY trap_entry_sync
    VECTOR_ENTRY trap_entry_irq
    VECTOR_ENTRY trap_entry_fiq
    VECTOR_ENTRY trap_entry_serror

trap_entry_sync:
trap_entry_irq:
trap_entry_fiq:
trap_entry_serror:
    /* Save x0-x30 onto the kernel stack (will be copied to TrapFrame) */
    sub sp, sp, #576          /* TrapFrame = 576 bytes */
    mov x1, sp                 /* x1 = TrapFrame pointer */

    /* Save GPRs x0-x30 to TrapFrame.gpr[0..30] (offsets 0..240) */
    stp x0, x1, [x1, #0]       /* gpr[0], gpr[1] — NOTE: x1 clobbered here */
    stp x2, x3, [x1, #16]
    stp x4, x5, [x1, #32]
    stp x6, x7, [x1, #48]
    stp x8, x9, [x1, #64]
    stp x10, x11, [x1, #80]
    stp x12, x13, [x1, #96]
    stp x14, x15, [x1, #112]
    stp x16, x17, [x1, #128]
    stp x18, x19, [x1, #144]
    stp x20, x21, [x1, #160]
    stp x22, x23, [x1, #176]
    stp x24, x25, [x1, #192]
    stp x26, x27, [x1, #208]
    stp x28, x29, [x1, #224]
    str x30, [x1, #240]        /* gpr[30] = x30 (lr) */

    /* Save system registers */
    mrs x2, sp_el0
    str x2, [x1, #288]         /* TrapFrame.sp = sp_el0 */
    mrs x2, elr_el1
    str x2, [x1, #256]         /* TrapFrame.pc = elr_el1 */
    mrs x2, spsr_el1
    str x2, [x1, #264]         /* TrapFrame.status = spsr_el1 */
    mrs x2, esr_el1
    str x2, [x1, #280]         /* TrapFrame.error_code = esr_el1 */
    /* TrapFrame.vector: derive from ESR_EL1 EC field (bits 31:26) */
    lsr x2, x2, #26
    str x2, [x1, #272]         /* TrapFrame.vector = EC */

    /* Set x0 = TrapFrame pointer for the VUMA call */
    mov x0, sp

    /* Call VUMA trap_handler(tf) */
    bl trap_handler

    /* trap_exit: restore from TrapFrame */
    .global trap_exit
trap_exit:
    mov x1, sp                  /* x1 = TrapFrame pointer */

    /* Restore GPRs */
    ldp x0, x1, [x1, #0]       /* NOTE: x1 clobbered — reload below */
    mov x1, sp
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

    /* Restore system registers */
    ldr x2, [x1, #288]
    msr sp_el0, x2
    ldr x2, [x1, #256]
    msr elr_el1, x2
    ldr x2, [x1, #264]
    msr spsr_el1, x2

    /* Release TrapFrame */
    add sp, sp, #576

    /* Exception return */
    eret
