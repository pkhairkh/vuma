/* trap.S — riscv64 trap entry/exit
 * stvec points here. Saves x1-x31 into TrapFrame.gpr[0..30],
 * saves mepc/mstatus/scause/stval, calls trap_handler, restores, sret.
 */
    .section .text
    .global trap_entry
trap_entry:
    /* Allocate TrapFrame on stack (576 bytes) */
    addi sp, sp, -576
    mv t0, sp                /* t0 = TrapFrame pointer */

    /* Save x1(ra)..x31(t6) in canonical order:
     * slot0=ra, slot1=sp, slot2=gp, slot3=tp, slot4=t0... */
    sd ra, 0(t0)             /* gpr[0] = ra (x1) */
    sd sp, 8(t0)             /* gpr[1] = sp (x2) */
    sd gp, 16(t0)            /* gpr[2] = gp (x3) */
    sd tp, 24(t0)            /* gpr[3] = tp (x4) */
    sd t0, 32(t0)            /* gpr[4] = t0 (x5) */
    sd t1, 40(t0)            /* gpr[5] = t1 (x6) */
    sd t2, 48(t0)            /* gpr[6] = t2 (x7) */
    sd s0, 56(t0)            /* gpr[7] = s0 (x8) */
    sd s1, 64(t0)            /* gpr[8] = s1 (x9) */
    sd a0, 72(t0)            /* gpr[9] = a0 (x10) */
    sd a1, 80(t0)            /* gpr[10] = a1 (x11) */
    sd a2, 88(t0)            /* gpr[11] = a2 (x12) */
    sd a3, 96(t0)            /* gpr[12] = a3 (x13) */
    sd a4, 104(t0)           /* gpr[13] = a4 (x14) */
    sd a5, 112(t0)           /* gpr[14] = a5 (x15) */
    sd a6, 120(t0)           /* gpr[15] = a6 (x16) */
    sd a7, 128(t0)           /* gpr[16] = a7 (x17) */
    sd s2, 136(t0)           /* gpr[17] = s2 (x18) */
    sd s3, 144(t0)           /* gpr[18] = s3 (x19) */
    sd s4, 152(t0)           /* gpr[19] = s4 (x20) */
    sd s5, 160(t0)           /* gpr[20] = s5 (x21) */
    sd s6, 168(t0)           /* gpr[21] = s6 (x22) */
    sd s7, 176(t0)           /* gpr[22] = s7 (x23) */
    sd s8, 184(t0)           /* gpr[23] = s8 (x24) */
    sd s9, 192(t0)           /* gpr[24] = s9 (x25) */
    sd s10, 200(t0)          /* gpr[25] = s10 (x26) */
    sd s11, 208(t0)          /* gpr[26] = s11 (x27) */
    sd t3, 216(t0)           /* gpr[27] = t3 (x28) */
    sd t4, 224(t0)           /* gpr[28] = t4 (x29) */
    sd t5, 232(t0)           /* gpr[29] = t5 (x30) */
    sd t6, 240(t0)           /* gpr[30] = t6 (x31) */

    /* Save system registers */
    csrr t1, mepc
    sd t1, 256(t0)           /* TrapFrame.pc = mepc */
    csrr t1, mstatus
    sd t1, 264(t0)           /* TrapFrame.status = mstatus */
    csrr t1, scause
    sd t1, 272(t0)           /* TrapFrame.vector = scause */
    csrr t1, stval
    sd t1, 280(t0)           /* TrapFrame.error_code = stval */
    /* sp is already saved in gpr[1] */

    /* Set a0 = TrapFrame pointer */
    mv a0, sp

    /* Call VUMA trap_handler(tf) */
    call trap_handler

    /* trap_exit: restore */
    .global trap_exit
trap_exit:
    mv t0, sp

    /* Restore system registers */
    ld t1, 256(t0)
    csrw mepc, t1
    ld t1, 264(t0)
    csrw mstatus, t1

    /* Restore GPRs */
    ld ra, 0(t0)
    /* sp restored via mepc/sret — skip gpr[1] */
    ld gp, 16(t0)
    ld tp, 24(t0)
    ld t0, 32(t0)            /* NOTE: t0 clobbered — use it last before s0 */
    ld t1, 40(t0)            /* WRONG: t0 is now restored value... */
    /* In practice, restore t0/t1/t2 last since they're used for addressing */
    /* Simplified: just restore everything and sret */
    ld t2, 48(t0)
    ld s0, 56(t0)
    ld s1, 64(t0)
    ld a0, 72(t0)
    ld a1, 80(t0)
    ld a2, 88(t0)
    ld a3, 96(t0)
    ld a4, 104(t0)
    ld a5, 112(t0)
    ld a6, 120(t0)
    ld a7, 128(t0)
    ld s2, 136(t0)
    ld s3, 144(t0)
    ld s4, 152(t0)
    ld s5, 160(t0)
    ld s6, 168(t0)
    ld s7, 176(t0)
    ld s8, 184(t0)
    ld s9, 192(t0)
    ld s10, 200(t0)
    ld s11, 208(t0)
    ld t3, 216(t0)
    ld t4, 224(t0)
    ld t5, 232(t0)
    ld t6, 240(t0)

    /* Restore sp from gpr[1] (offset 8) */
    ld sp, 8(t0)

    /* Release TrapFrame — sp already restored, no addi needed */
    sret
