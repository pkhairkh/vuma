/* boot.S — x86_64 bare-metal boot entry (Multiboot2)
 *
 * QEMU: qemu-system-x86_64 -kernel kernel.bin -serial stdio
 *
 * TrapFrame canonical GPR slot mapping (x86_64):
 *   slot 0  = rax (TrapFrame.gpr offset 0)
 *   slot 1  = rbx (offset 8)
 *   slot 2  = rcx (offset 16)
 *   slot 3  = rdx (offset 24)
 *   slot 4  = rsi (offset 32)
 *   slot 5  = rdi (offset 40)
 *   slot 6  = rbp (offset 48)
 *   slot 7  = r8  (offset 56)
 *   slot 8  = r9  (offset 64)
 *   slot 9  = r10 (offset 72)
 *   slot 10 = r11 (offset 80)
 *   slot 11 = r12 (offset 88)
 *   slot 12 = r13 (offset 96)
 *   slot 13 = r14 (offset 104)
 *   slot 14 = r15 (offset 112)
 *   TrapFrame.pc     = rip   (offset 256)
 *   TrapFrame.status = rflags (offset 264)
 *   TrapFrame.vector = vector (offset 272)
 *   TrapFrame.error_code = error (offset 280)
 *   TrapFrame.sp     = rsp   (offset 288)
 */

    .section .multiboot
    .align 8
multiboot_header:
    .long 0xE85250D6          /* magic */
    .long 0                    /* architecture: i386 */
    .long multiboot_header_end - multiboot_header  /* header length */
    .long 0x100000000 - 0xE85250D6 - (multiboot_header_end - multiboot_header)  /* checksum */
    /* end tag */
    .short 0
    .short 0
    .long 8
multiboot_header_end:

    .section .text
    .global _start
_start:
    /* Set up stack */
    movq $0x90000, %rsp

    /* Zero BSS */
    leaq bss_start(%rip), %rdi
    leaq bss_end(%rip), %rcx
    xorq %rax, %rax
1:
    cmpq %rcx, %rdi
    jge 2f
    movq %rax, (%rdi)
    addq $8, %rdi
    jmp 1b
2:

    /* rdi = multiboot2 magic (already in eax), rsi = multiboot2 info */
    /* VUMA main() takes no args in the PMT model; boot stub handles BootInfo */

    /* Call VUMA main() */
    call main

    /* Halt on return */
3:
    cli
    hlt
    jmp 3b

    .section .bss
    .align 16
bss_start:
    .skip 65536
bss_end:
