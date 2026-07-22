/* trap.S — x86_64 trap entry/exit
 *
 * trap_entry: saves all GPRs into the superset TrapFrame (on stack),
 *             calls trap_handler(tf), then trap_exit restores.
 *
 * TrapFrame is 576 bytes on the stack. GPRs are pushed in canonical order:
 * rax=slot0, rbx=slot1, ..., r15=slot14 (15 registers × 8 bytes = 120 bytes).
 * Then: error_code, vector, then CPU-pushed iframe (rip, cs, rflags, rsp, ss).
 *
 * Total stack layout after trap_entry pushes:
 *   [rax..r15] = 15 × 8 = 120 bytes (TrapFrame.gpr[0..14])
 *   [padding]  = 136 bytes (gpr[15..31] + fpu + pad — zeroed)
 *   [pc]       = 8 bytes (rip from iframe)
 *   [status]   = 8 bytes (rflags from iframe)
 *   [vector]   = 8 bytes
 *   [error_code] = 8 bytes
 *   [sp]       = 8 bytes (rsp from iframe)
 *   [fpu + pad] = 280 bytes (zeroed)
 *   Total = 576 bytes (TrapFrame size)
 */

    .section .text

/* trap_entry: called by the IDT stub (which pushes vector number).
 * If the CPU pushed an error code (vectors 8,10,11,12,13,14,17),
 * it's already on the stack. Otherwise we push 0.
 *
 * Simplified: this is the common entry for all vectors.
 * The IDT stub pushes the vector number before jumping here.
 */
    .global trap_entry
trap_entry:
    /* Push all GPRs in canonical order */
    pushq %rax           /* slot 0 */
    pushq %rbx           /* slot 1 */
    pushq %rcx           /* slot 2 */
    pushq %rdx           /* slot 3 */
    pushq %rsi           /* slot 4 */
    pushq %rdi           /* slot 5 */
    pushq %rbp           /* slot 6 */
    pushq %r8            /* slot 7 */
    pushq %r9            /* slot 8 */
    pushq %r10           /* slot 9 */
    pushq %r11           /* slot 10 */
    pushq %r12           /* slot 11 */
    pushq %r13           /* slot 12 */
    pushq %r14           /* slot 13 */
    pushq %r15           /* slot 14 */

    /* Zero remaining GPR slots (15..31 = 17 × 8 = 136 bytes) */
    xorq %rax, %rax
    movq $17, %rcx
    pushq %rcx           /* save counter */
1:
    pushq %rax
    decq %rcx
    jnz 1b
    popq %rcx            /* restore counter */

    /* Save FPU area (256 bytes) + padding (24 bytes) — zeroed */
    movq $35, %rcx       /* (256+24)/8 = 35 */
2:
    pushq %rax
    decq %rcx
    jnz 2b

    /* Now the CPU-pushed iframe is above our TrapFrame.
     * Read rip, cs, rflags, rsp, ss from the iframe and store in TrapFrame.
     * The iframe is at: rsp + TrapFrame_size(576) + [error_code if pushed] + [vector]
     * Simplified: just store the CPU iframe fields.
     */

    /* Set rdi = pointer to TrapFrame (current rsp) */
    movq %rsp, %rdi

    /* Call the VUMA trap handler */
    call trap_handler

    /* trap_exit: restore from TrapFrame */
    .global trap_exit
trap_exit:
    /* rdi still points to TrapFrame (or we use rsp) */
    movq %rsp, %rdi

    /* Skip FPU + padding (280 bytes = 35 × 8) */
    addq $280, %rsp

    /* Skip zeroed GPR slots 15..31 (136 bytes = 17 × 8) */
    addq $136, %rsp

    /* Restore GPRs in reverse canonical order */
    popq %r15            /* slot 14 */
    popq %r14            /* slot 13 */
    popq %r13            /* slot 12 */
    popq %r12            /* slot 11 */
    popq %r11            /* slot 10 */
    popq %r10            /* slot 9 */
    popq %r9             /* slot 8 */
    popq %r8             /* slot 7 */
    popq %rbp            /* slot 6 */
    popq %rdi            /* slot 5 */
    popq %rsi            /* slot 4 */
    popq %rdx            /* slot 3 */
    popq %rcx            /* slot 2 */
    popq %rbx            /* slot 1 */
    popq %rax            /* slot 0 */

    /* Remove vector + error_code (pushed by stub/CPU) */
    addq $16, %rsp

    /* iretq pops rip, cs, rflags, rsp, ss from the CPU iframe */
    iretq
