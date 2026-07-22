/* boot.S — Hosted-mode boot entry (Linux process)
 *
 * This is the simplest boot stub: the kernel runs as a Linux process.
 * The VUMA compile_dump already emits a _start stub that calls main() and
 * exits with the return value. This file is a placeholder for the hosted
 * framework — it just calls main and exits.
 *
 * Used by: wasm32, alpha, hppa, m68k, sparc64, x86_32, aarch64_be, armeb,
 * mips64be, ppc64 (10 backends without bare-metal QEMU support in this env).
 */

    .section .text
    .global _start
_start:
    /* The VUMA compile_dump _start stub already handles this:
     *   mov rdi, [rsp]      ; argc
     *   lea rsi, [rsp+8]    ; argv
     *   call main
     *   mov rdi, rax        ; exit code = main's return
     *   mov rax, 60         ; sys_exit
     *   syscall
     *
     * This file exists for documentation and as a link target when
     * building a "hosted kernel" that runs as a Linux process.
     */
    call main
    mov %rax, %rdi
    mov $60, %rax
    syscall
