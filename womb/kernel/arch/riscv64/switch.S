/* switch.S — riscv64 context switch
 * arch_load_context: loads registers from the superset TrapFrame.
 * a0 = pointer to TrapFrame (RISC-V ABI first arg).
 */
    .section .text
    .global arch_load_context
arch_load_context:
    /* Save a0 (TrapFrame pointer) in t0 */
    mv t0, a0

    /* Load sp from offset 288 */
    ld sp, 288(t0)

    /* Load mepc from offset 256 */
    ld t1, 256(t0)
    csrw mepc, t1

    /* Load mstatus from offset 264 */
    ld t1, 264(t0)
    csrw mstatus, t1

    /* Load GPRs (skip sp=gpr[1] — already loaded above) */
    ld ra, 0(t0)             /* gpr[0] = ra */
    ld gp, 16(t0)            /* gpr[2] = gp */
    ld tp, 24(t0)            /* gpr[3] = tp */
    ld t1, 40(t0)            /* gpr[5] = t1 */
    ld t2, 48(t0)            /* gpr[6] = t2 */
    ld s0, 56(t0)            /* gpr[7] = s0 */
    ld s1, 64(t0)            /* gpr[8] = s1 */
    ld a0, 72(t0)            /* gpr[9] = a0 — must be before a1-a7 */
    ld a1, 80(t0)            /* gpr[10] = a1 */
    ld a2, 88(t0)            /* gpr[11] = a2 */
    ld a3, 96(t0)            /* gpr[12] = a3 */
    ld a4, 104(t0)           /* gpr[13] = a4 */
    ld a5, 112(t0)           /* gpr[14] = a5 */
    ld a6, 120(t0)           /* gpr[15] = a6 */
    ld a7, 128(t0)           /* gpr[16] = a7 */
    ld s2, 136(t0)           /* gpr[17] = s2 */
    ld s3, 144(t0)           /* gpr[18] = s3 */
    ld s4, 152(t0)           /* gpr[19] = s4 */
    ld s5, 160(t0)           /* gpr[20] = s5 */
    ld s6, 168(t0)           /* gpr[21] = s6 */
    ld s7, 176(t0)           /* gpr[22] = s7 */
    ld s8, 184(t0)           /* gpr[23] = s8 */
    ld s9, 192(t0)           /* gpr[24] = s9 */
    ld s10, 200(t0)          /* gpr[25] = s10 */
    ld s11, 208(t0)          /* gpr[26] = s11 */
    ld t3, 216(t0)           /* gpr[27] = t3 */
    ld t4, 224(t0)           /* gpr[28] = t4 */
    ld t5, 232(t0)           /* gpr[29] = t5 */
    ld t6, 240(t0)           /* gpr[30] = t6 */

    /* Load t0 from gpr[4] (offset 32) — must be last */
    ld t0, 32(t0)

    /* sret: loads mepc into pc, mstatus into pstate */
    sret
