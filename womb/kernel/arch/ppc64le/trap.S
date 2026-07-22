/* trap.S — ppc64le trap entry/exit
 * Saves r0-r31 into TrapFrame.gpr[0..31], saves pc(lr)/msr/srr1,
 * calls trap_handler, restores + rfid.
 */
    .section .text
    .global trap_entry
trap_entry:
    /* Allocate TrapFrame on stack (576 bytes) */
    stdu sp, -576(sp)
    mr r12, sp              /* r12 = TrapFrame pointer */

    /* Save GPRs r0-r31 to gpr[0..31] (offsets 0..248) */
    std r0, 0(r12)
    std r1, 8(r12)          /* NOTE: r1 = sp, but we saved it above */
    std r2, 16(r12)
    std r3, 24(r12)
    std r4, 32(r12)
    std r5, 40(r12)
    std r6, 48(r12)
    std r7, 56(r12)
    std r8, 64(r12)
    std r9, 72(r12)
    std r10, 80(r12)
    std r11, 88(r12)
    std r12, 96(r12)
    std r13, 104(r12)
    std r14, 112(r12)
    std r15, 120(r12)
    std r16, 128(r12)
    std r17, 136(r12)
    std r18, 144(r12)
    std r19, 152(r12)
    std r20, 160(r12)
    std r21, 168(r12)
    std r22, 176(r12)
    std r23, 184(r12)
    std r24, 192(r12)
    std r25, 200(r12)
    std r26, 208(r12)
    std r27, 216(r12)
    std r28, 224(r12)
    std r29, 232(r12)
    std r30, 240(r12)
    std r31, 248(r12)

    /* Save system registers */
    mfsrr0 r0               /* exception PC (from SRR0) */
    std r0, 256(r12)        /* TrapFrame.pc */
    mfsrr1 r0               /* saved MSR (from SRR1) */
    std r0, 264(r12)        /* TrapFrame.status */
    /* vector + error_code not directly available on POWER — use dummy */
    li r0, 0
    std r0, 272(r12)        /* TrapFrame.vector = 0 (stub) */
    std r0, 280(r12)        /* TrapFrame.error_code = 0 */
    std r1, 288(r12)        /* TrapFrame.sp = saved sp */

    /* Set r3 = TrapFrame pointer */
    mr r3, sp

    /* Call VUMA trap_handler(tf) */
    bl trap_handler

    /* trap_exit */
    .global trap_exit
trap_exit:
    mr r12, sp

    /* Restore system registers */
    ld r0, 256(r12)
    mtsrr0 r0               /* restore exception PC */
    ld r0, 264(r12)
    mtsrr1 r0               /* restore MSR */

    /* Restore GPRs */
    ld r0, 0(r12)
    ld r2, 16(r12)
    ld r3, 24(r12)
    ld r4, 32(r12)
    ld r5, 40(r12)
    ld r6, 48(r12)
    ld r7, 56(r12)
    ld r8, 64(r12)
    ld r9, 72(r12)
    ld r10, 80(r12)
    ld r11, 88(r12)
    ld r13, 104(r12)
    ld r14, 112(r12)
    ld r15, 120(r12)
    ld r16, 128(r12)
    ld r17, 136(r12)
    ld r18, 144(r12)
    ld r19, 152(r12)
    ld r20, 160(r12)
    ld r21, 168(r12)
    ld r22, 176(r12)
    ld r23, 184(r12)
    ld r24, 192(r12)
    ld r25, 200(r12)
    ld r26, 208(r12)
    ld r27, 216(r12)
    ld r28, 224(r12)
    ld r29, 232(r12)
    ld r30, 240(r12)
    ld r31, 248(r12)

    /* Restore sp */
    ld r1, 8(r12)

    /* rfid: loads SRR0 into PC, SRR1 into MSR */
    rfid
