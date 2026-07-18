/* switch.S — x86_64 context switch
 *
 * arch_load_context: loads registers from the superset TrapFrame.
 * rdi = pointer to TrapFrame (System V AMD64 ABI first arg).
 *
 * TrapFrame layout:
 *   gpr[0]  (offset 0)   = rax
 *   gpr[1]  (offset 8)   = rbx
 *   gpr[2]  (offset 16)  = rcx
 *   gpr[3]  (offset 24)  = rdx
 *   gpr[4]  (offset 32)  = rsi
 *   gpr[5]  (offset 40)  = rdi
 *   gpr[6]  (offset 48)  = rbp
 *   gpr[7]  (offset 56)  = r8
 *   gpr[8]  (offset 64)  = r9
 *   gpr[9]  (offset 72)  = r10
 *   gpr[10] (offset 80)  = r11
 *   gpr[11] (offset 88)  = r12
 *   gpr[12] (offset 96)  = r13
 *   gpr[13] (offset 104) = r14
 *   gpr[14] (offset 112) = r15
 *   pc      (offset 256) = rip
 *   status  (offset 264) = rflags
 *   sp      (offset 288) = rsp
 */

    .section .text
    .global arch_load_context

arch_load_context:
    /* rdi = pointer to TrapFrame */

    /* Load GPRs from TrapFrame (canonical order) */
    movq 0(%rdi), %rax       /* slot 0 = rax */
    movq 8(%rdi), %rbx       /* slot 1 = rbx */
    movq 16(%rdi), %rcx      /* slot 2 = rcx */
    movq 24(%rdi), %rdx      /* slot 3 = rdx */
    movq 32(%rdi), %rsi      /* slot 4 = rsi */
    /* slot 5 = rdi — load last (we need rdi for addressing) */
    movq 48(%rdi), %rbp      /* slot 6 = rbp */
    movq 56(%rdi), %r8       /* slot 7 = r8 */
    movq 64(%rdi), %r9       /* slot 8 = r9 */
    movq 72(%rdi), %r10      /* slot 9 = r10 */
    movq 80(%rdi), %r11      /* slot 10 = r11 */
    movq 88(%rdi), %r12      /* slot 11 = r12 */
    movq 96(%rdi), %r13      /* slot 12 = r13 */
    movq 104(%rdi), %r14     /* slot 13 = r14 */
    movq 112(%rdi), %r15     /* slot 14 = r15 */

    /* Load sp from offset 288 */
    movq 288(%rdi), %rsp

    /* Push the return address (pc from offset 256) onto the new stack */
    pushq 256(%rdi)

    /* Load rflags from offset 264 */
    pushq 264(%rdi)
    popfq

    /* Load rdi from slot 5 (offset 40) — must be last */
    movq 40(%rdi), %rdi      /* NOTE: this clobbers the TrapFrame pointer */

    /* ret pops the pushed pc into rip */
    ret
