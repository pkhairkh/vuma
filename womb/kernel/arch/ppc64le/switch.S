/* switch.S — ppc64le context switch
 * arch_load_context: loads registers from the superset TrapFrame.
 * r3 = pointer to TrapFrame (PowerPC ABI first arg).
 */
    .section .text
    .global arch_load_context
arch_load_context:
    mr r12, r3               /* save TrapFrame pointer */

    /* Load sp from offset 288 */
    ld r1, 288(r12)

    /* Load SRR0 (pc) from offset 256 */
    ld r0, 256(r12)
    mtsrr0 r0

    /* Load SRR1 (msr) from offset 264 */
    ld r0, 264(r12)
    mtsrr1 r0

    /* Load GPRs r0, r2-r31 */
    ld r0, 0(r12)
    ld r2, 16(r12)
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

    /* Load r3 from offset 24 — must be last */
    ld r3, 24(r12)

    rfid
