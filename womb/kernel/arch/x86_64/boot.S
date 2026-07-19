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
 *
 * Boot sequence (16-bit real → 32-bit prot → 64-bit long):
 *   1. Multiboot2 loader hands control to _start in 32-bit protected mode.
 *   2. Set up stack, load GDT (null + 64-bit code + 64-bit data).
 *   3. Far jump to 64-bit code segment (CS=0x08) — still in 32-bit paging-off.
 *   4. Disable paging, enable PAE, build identity-mapped 4-level page tables
 *      (PML4 → PDPT → PD with 512 × 2MB huge pages = first 1GB identity-mapped).
 *   5. Load CR3, set LME in EFER MSR, re-enable paging in CR0.
 *   6. Far jump to 64-bit code segment again — now CPU is in long mode.
 *   7. Call VUMA main(); halt on return.
 */

.set MB2_HEADER_MAGIC, 0xE85250D6
.set MB2_ARCH_TYPE, 0    /* i386 */
.set MB2_HEADER_LENGTH, 24
.set MB2_CHECKSUM, -(MB2_HEADER_MAGIC + 0 + MB2_HEADER_LENGTH)

.section .multiboot
.align 8
mb2_header:
    .long MB2_HEADER_MAGIC
    .long MB2_ARCH_TYPE
    .long MB2_HEADER_LENGTH
    .long MB2_CHECKSUM
    /* End tag */
    .short 0
    .short 0
    .long 8

.section .bss
.align 16
stack_bottom:
    .skip 16384  /* 16 KB stack */
stack_top:

.align 4096
pml4_table:
    .skip 4096
pdpt_table:
    .skip 4096
pd_table:
    .skip 4096

.section .text
.global _start
.type _start, @function
_start:
    /* Set up stack */
    mov $stack_top, %esp

    /* Save multiboot info (eax=magic, ebx=mbi_ptr) */
    mov %eax, %edi
    mov %ebx, %esi

    /* Set up GDT */
    lgdt gdt_descriptor

    /* Load segment registers */
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %fs
    mov %ax, %gs
    mov %ax, %ss

    /* Far jump to 64-bit code segment */
    ljmp $0x08, $long_mode_setup

long_mode_setup:
    /* Disable paging */
    mov %cr0, %eax
    and $0x7FFFFFFF, %eax
    mov %eax, %cr0

    /* Enable PAE */
    mov %cr4, %eax
    or $1 << 5, %eax
    mov %eax, %cr4

    /* Set up identity-mapped page tables (first 1GB) */
    /* PML4[0] -> PDPT */
    mov $(pdpt_table), %eax
    or $0x3, %eax  /* present + writable */
    mov %eax, (pml4_table)

    /* PDPT[0] -> PD */
    mov $(pd_table), %eax
    or $0x3, %eax
    mov %eax, (pdpt_table)

    /* PD[0..511] -> 2MB pages (identity map first 1GB) */
    mov $pd_table, %edi
    mov $0x83, %eax  /* present + writable + huge page */
    mov $512, %ecx
1:
    mov %eax, (%edi)
    add $8, %edi
    add $0x200000, %eax  /* next 2MB */
    loop 1b

    /* Load CR3 */
    mov $(pml4_table), %eax
    mov %eax, %cr3

    /* Set LME bit in EFER MSR */
    mov $0xC0000080, %ecx
    rdmsr
    or $1 << 8, %eax
    wrmsr

    /* Enable paging */
    mov %cr0, %eax
    or $0x80000000, %eax
    mov %eax, %cr0

    /* Now in 64-bit mode! Jump to 64-bit code */
    ljmp $0x08, $long_mode_start

long_mode_start:
    /* Call kmain (the VUMA main function) */
    call main

    /* Halt on return */
    cli
2:
    hlt
    jmp 2b

/* GDT */
.section .rodata
gdt:
    .quad 0  /* null descriptor */
    /* 64-bit code segment */
    .word 0
    .word 0
    .byte 0
    .byte 0x9A  /* present, code, execute/read */
    .byte 0xAF  /* 64-bit, granularity */
    .byte 0
    /* 64-bit data segment */
    .word 0
    .word 0
    .byte 0
    .byte 0x92  /* present, data, read/write */
    .byte 0xAF  /* 64-bit, granularity */
    .byte 0

gdt_descriptor:
    .word gdt_descriptor - gdt - 1
    .quad gdt
